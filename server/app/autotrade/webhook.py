"""Inbound TradingView / external signal webhook trading bot (MT-10).

Security & safety rails:
- Per-bot encrypted 24-byte random secret verified with `hmac.compare_digest` (via `X-Webhook-Secret` header or JSON `secret`)
- Sliding-window rate limit per `(bot_id, client_ip)` (default 15 req / 60s -> 429)
- Idempotency deduplication on `idempotency_key` / `alert_id` / `bar_time:action`
- Respects global kill switch (`autotrade_halted`), `bot["enabled"]`, circuit breakers (`guard`),
  `capital` ceiling, Toss `buying-power` / `sellable-quantity`, and `trading.check_limits`.
"""
from __future__ import annotations

import hashlib
import hmac
import math
import secrets
import time
import uuid
from datetime import datetime, timedelta, timezone
from typing import Any, Optional

from flask import g

from ..account.toss_account import _account, normalize
from ..crypto import decrypt, encrypt
from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from ..quant.data import cost_defaults
from ..storage import get_store
from ..trading import orders as trading
from ..trading.ticks import fmt_price, round_price
from ..users import credentials
from ..users import repository as users
from . import bots, executor, protections
from .scheduler import active_session

RATE_WINDOW_SEC = 60.0
RATE_MAX_CALLS = 15
LIVE_BUY_HEADROOM = 0.005

_rate_buckets: dict[tuple[str, str], list[float]] = {}


def reset_rate_limits() -> None:
    _rate_buckets.clear()


def _check_rate_limit(bot_id: str, remote_ip: str) -> None:
    now = time.monotonic()
    key = (bot_id, remote_ip or "unknown")
    hits = [t for t in _rate_buckets.get(key, []) if now - t < RATE_WINDOW_SEC]
    if len(hits) >= RATE_MAX_CALLS:
        _rate_buckets[key] = hits
        raise ApiError(429, "webhook_rate_limited", "웹훅 요청이 너무 많습니다 (분당 최대 15회)")
    hits.append(now)
    _rate_buckets[key] = hits


def generate_secret_fields() -> tuple[str, dict[str, str]]:
    raw = secrets.token_urlsafe(24)
    return raw, {
        "webhook_secret_enc": encrypt(raw),
        "webhook_secret_hint": f"{raw[:4]}…{raw[-4:]}",
    }


def get_or_rotate_secret(bot: dict, rotate: bool = False) -> str:
    if bots.bot_kind(bot) != "webhook":
        raise ApiError(400, "not_webhook_bot", "웹훅 봇에서만 비밀 토큰을 관리할 수 있습니다")
    enc = bot.get("webhook_secret_enc")
    if enc and not rotate:
        try:
            return decrypt(enc)
        except Exception:  # noqa: BLE001
            pass
    raw, fields = generate_secret_fields()
    bot.update(fields)
    bots.log(bot, "config", "웹훅 비밀 토큰이 새로 발급되었습니다" if rotate else "웹훅 비밀 토큰을 확인했습니다")
    bots.save(bot)
    return raw


def _verify_secret(bot: dict, provided: str) -> None:
    enc = bot.get("webhook_secret_enc")
    if not enc or not provided:
        raise ApiError(403, "invalid_webhook_secret", "웹훅 비밀 토큰이 올바르지 않습니다")
    try:
        expected = decrypt(enc)
    except Exception as e:  # noqa: BLE001
        raise ApiError(403, "invalid_webhook_secret", "웹훅 비밀 토큰 검증에 실패했습니다") from e
    if not hmac.compare_digest(expected.encode("utf-8"), provided.strip().encode("utf-8")):
        raise ApiError(403, "invalid_webhook_secret", "웹훅 비밀 토큰이 일치하지 않습니다")


def _cid(bot_id: str, sig_id: str) -> str:
    return hashlib.sha256(f"{bot_id}:wh:{sig_id}".encode()).hexdigest()[:32]


def _resolve_reference_price(bot: dict, payload_price: Optional[float], use_toss: bool) -> float:
    if use_toss:
        try:
            client, _ = _account()
            rows = client.request("GET", "/api/v1/prices", params={"symbols": bot["symbol"]}) or []
            if rows and rows[0].get("lastPrice") is not None:
                return float(rows[0]["lastPrice"])
        except Exception:  # noqa: BLE001
            pass
    if payload_price and payload_price > 0:
        return float(payload_price)
    market = Market(bot["market"])
    provider = get_provider(market)
    try:
        q = provider.get_quote(market, bot["symbol"])
        if q and q.price > 0:
            return float(q.price)
    except Exception:  # noqa: BLE001
        pass
    cs = provider.get_candles(market, bot["symbol"], Interval.D1, limit=5)
    if cs:
        return float(cs[-1].close)
    raise ApiError(400, "price_unavailable", "현재 시세를 확인할 수 없습니다 (웹훅 JSON에 price를 함께 보내거나 시세 연결을 확인하세요)")


def preview(bot: dict, today: str) -> dict[str, Any]:
    st = bots.state(bot)
    pos = st["position"]
    last_px = st.get("last_price") or pos.get("entry_price") or 0.0
    if not last_px:
        try:
            last_px = _resolve_reference_price(bot, None, use_toss=False)
        except Exception:  # noqa: BLE001
            last_px = 0.0
    cash = float(st["cash"]) if bot["mode"] == "paper" else max(float(bot["capital"]) - pos["qty"] * pos["entry_price"], 0.0)
    return {
        "date": today,
        "bar_date": st.get("last_run_date") or today,
        "due": False,
        "reason": "외부 웹훅(TradingView 등) 신호 수신 대기 중",
        "position": pos,
        "cash": cash,
        "today_open": None,
        "last": last_px,
        "orders": [],
    }


def receive_webhook(bot_id: str, body: dict[str, Any], header_secret: Optional[str], remote_ip: str) -> dict[str, Any]:
    _check_rate_limit(bot_id, remote_ip)
    bot = get_store().get(bots.BOTS, bot_id)
    if not bot or bots.bot_kind(bot) != "webhook":
        raise ApiError(404, "bot_not_found", "웹훅 봇을 찾을 수 없습니다")

    provided_secret = header_secret or str(body.get("secret") or "")
    _verify_secret(bot, provided_secret)

    user = users.get_user(bot["user_id"])
    if not user or users.status_of(user) != "approved":
        raise ApiError(403, "user_not_approved", "승인된 계정이 아닙니다")
    if user.get("autotrade_halted"):
        bots.log(bot, "skip", "[웹훅] 전체 비상 중지 상태라 수신한 신호를 무시했습니다", "warn")
        raise ApiError(409, "autotrade_halted", "자동매매 전체 중지 상태입니다")
    if not bot.get("enabled"):
        bots.log(bot, "skip", "[웹훅] 봇이 꺼져 있어 수신한 신호를 실행하지 않았습니다", "warn")
        raise ApiError(409, "bot_disabled", "웹훅 봇이 꺼져 있습니다. 먼저 봇을 켜 주세요")

    g.user = user

    action_raw = str(body.get("action") or body.get("side") or "").strip().lower()
    if action_raw in ("exit", "close", "flat"):
        action = "exit"
    elif action_raw in ("buy", "long"):
        action = "buy"
    elif action_raw in ("sell", "short_exit"):
        action = "sell"
    else:
        raise ApiError(400, "invalid_action", "action은 buy / sell / exit 중 하나여야 합니다")

    if body.get("symbol"):
        incoming_sym = str(body["symbol"]).strip().upper().split(":")[-1]
        if incoming_sym and incoming_sym != bot["symbol"]:
            raise ApiError(400, "symbol_mismatch", f"봇 종목({bot['symbol']})과 웹훅 종목({incoming_sym})이 다릅니다")

    try:
        qty_pct = float(body.get("qty_pct", 100.0))
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_qty_pct", "qty_pct는 1 ~ 100 사이 숫자여야 합니다")
    if not 1.0 <= qty_pct <= 100.0:
        raise ApiError(400, "invalid_qty_pct", "qty_pct는 1 ~ 100 사이 숫자여야 합니다")

    order_type = str(body.get("order_type") or "market").strip().lower()
    if order_type not in ("market", "limit"):
        raise ApiError(400, "invalid_order_type", "order_type은 market 또는 limit이어야 합니다")

    payload_price: Optional[float] = None
    if body.get("price") is not None and str(body.get("price")).strip() != "":
        try:
            payload_price = float(body["price"])
        except (TypeError, ValueError):
            raise ApiError(400, "invalid_price", "price는 숫자여야 합니다")
        if payload_price <= 0:
            raise ApiError(400, "invalid_price", "price는 0보다 커야 합니다")
    if order_type == "limit" and payload_price is None:
        raise ApiError(400, "price_required", "지정가(limit) 웹훅 주문에는 price 값이 필요합니다")

    reason = str(body.get("reason") or body.get("comment") or f"외부 웹훅 ({action.upper()})").strip()[:100]
    idem = str(
        body.get("idempotency_key")
        or body.get("alert_id")
        or (f"{body['bar_time']}:{action}" if body.get("bar_time") else "")
    ).strip()[:80]

    st = bots.state(bot)
    seen = st.setdefault("webhook_seen", [])
    if idem and idem in seen:
        return {"ok": True, "deduped": True, "action": action}
    if idem:
        seen.append(idem)
        if len(seen) > 200:
            del seen[:-200]

    today = trading.kst_today()
    mode = bot["mode"]
    sig_id = idem or uuid.uuid4().hex[:12]

    # 1) PAPER MODE
    if mode == "paper":
        ref_px = _resolve_reference_price(bot, payload_price, use_toss=False)
        c = executor._costs(Market(bot["market"]))
        pos = st["position"]
        if action == "buy":
            buy_block = protections.check_buy_allowed(bot, st, today, current_equity=st["cash"] + pos["qty"] * ref_px)
            if buy_block:
                bots.log(bot, "skip", f"[모의/웹훅] 신규 매수 보류: {buy_block}", "warn")
                bots.save(bot)
                return {"ok": True, "skipped": buy_block}
            px = ref_px * (1.0 + c.slippage_pct / 100.0)
            unit = px * (1.0 + c.commission_pct / 100.0)
            spend = float(st["cash"]) * (qty_pct / 100.0)
            qty = math.floor(spend / unit) if unit > 0 else 0
            if qty <= 0:
                bots.log(bot, "skip", "[모의/웹훅] 가용 현금 부족으로 매수하지 못했습니다", "warn")
                bots.save(bot)
                return {"ok": True, "skipped": "insufficient_cash"}
            if pos["qty"] == 0:
                executor._paper_buy(st, qty, ref_px, c, today, reason)
            else:
                cost = qty * unit
                st["cash"] -= cost
                new_q = pos["qty"] + qty
                avg = (pos["qty"] * pos["entry_price"] + qty * px) / new_q
                st["position"] = {"qty": new_q, "entry_price": avg, "entry_date": pos["entry_date"] or today}
                st["peak_price"] = max(st.get("peak_price") or avg, px)
            bots.log(bot, "fill", f"[모의/웹훅] 매수 체결 {qty}주 @ {px:,.2f} — {reason}")
            res_qty = qty
        else:
            if pos["qty"] <= 0:
                bots.log(bot, "skip", "[모의/웹훅] 보유 수량이 없어 매도 신호를 건너뛰었습니다", "info")
                bots.save(bot)
                return {"ok": True, "skipped": "no_position"}
            sell_q = pos["qty"] if (action == "exit" or qty_pct >= 99.9) else max(1, math.floor(pos["qty"] * qty_pct / 100.0))
            sell_q = min(sell_q, pos["qty"])
            if sell_q >= pos["qty"]:
                executor._paper_sell(st, ref_px, c, today, reason)
            else:
                px = ref_px * (1.0 - c.slippage_pct / 100.0)
                net = sell_q * px * (1.0 - (c.commission_pct + c.tax_pct) / 100.0)
                st["cash"] += net
                st["position"] = {"qty": pos["qty"] - sell_q, "entry_price": pos["entry_price"], "entry_date": pos["entry_date"]}
            bots.log(bot, "fill", f"[모의/웹훅] 매도 체결 {sell_q}주 @ {ref_px:,.2f} — {reason}")
            res_qty = sell_q

        st["last_run_date"] = today
        st["last_price"] = ref_px
        executor._record_day_equity(st, today, st["cash"] + st["position"]["qty"] * ref_px)
        protections.evaluate_after_step(bot, st, today)
        bots.save(bot)
        return {"ok": True, "mode": "paper", "action": action, "qty": res_qty, "price": ref_px}

    # 2) SEMI & LIVE MODES (require Toss credentials)
    if not credentials.get_toss(user["id"]):
        raise ApiError(409, "toss_credentials_required", "토스증권 API 키가 등록되어 있지 않습니다")

    executor.reconcile_live(bot, today)
    client, seq = _account()
    m = bot["market"]
    currency = "KRW" if m == "KR" else "USD"
    d_cost = cost_defaults(Market(m))
    headroom = max((d_cost["slippage_pct"] + d_cost["commission_pct"]) / 100.0, LIVE_BUY_HEADROOM)
    ref_px = _resolve_reference_price(bot, payload_price, use_toss=True)
    pos = st["position"]

    if action == "buy":
        buy_block = protections.check_buy_allowed(
            bot, st, today, current_equity=float(bot["capital"]) + st.get("realized", 0.0) + pos["qty"] * (ref_px - pos["entry_price"])
        )
        if buy_block:
            bots.log(bot, "skip", f"[웹훅] 신규 매수 보류: {buy_block}", "warn")
            bots.save(bot)
            return {"ok": True, "skipped": buy_block}
        bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
        cash_bp = float(bp.get("cashBuyingPower") or 0.0)
        rem_cap = max(float(bot["capital"]) - pos["qty"] * pos["entry_price"], 0.0)
        budget = min(rem_cap, cash_bp) * (qty_pct / 100.0)
        est_px = float(payload_price) if (order_type == "limit" and payload_price) else ref_px
        qty = math.floor(budget / (est_px * (1.0 + headroom))) if est_px > 0 else 0
        side = "BUY"
    else:
        if pos["qty"] <= 0:
            bots.log(bot, "skip", "[웹훅] 봇 보유 수량이 없어 매도 신호를 건너뛰었습니다", "info")
            bots.save(bot)
            return {"ok": True, "skipped": "no_position"}
        sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": bot["symbol"]},
                                         account_seq=seq) or {}).get("sellableQuantity") or 0)
        want_q = pos["qty"] if (action == "exit" or qty_pct >= 99.9) else max(1, math.floor(pos["qty"] * qty_pct / 100.0))
        qty = int(min(want_q, pos["qty"], sellable))
        est_px = float(payload_price) if (order_type == "limit" and payload_price) else ref_px
        side = "SELL"

    if qty <= 0:
        bots.log(bot, "skip", f"[웹훅] {'매수 가능 자금' if side == 'BUY' else '매도 가능 수량'} 부족으로 주문을 생략했습니다", "warn")
        bots.save(bot)
        return {"ok": True, "skipped": "zero_qty"}

    if mode == "semi":
        now_dt = datetime.now(timezone.utc)
        sess = active_session(m, now_dt)
        exp_dt = sess[2] if sess else (now_dt + timedelta(hours=4))
        app_id = uuid.uuid4().hex[:12]
        st["pending_approval"] = {
            "id": app_id,
            "date": today,
            "created_at": now_dt.isoformat(),
            "expires_at": exp_dt.isoformat(),
            "reason": f"[웹훅] {reason}",
            "orders": [{
                "symbol": bot["symbol"],
                "side": side,
                "kind": order_type,
                "qty": qty,
                "signal_price": round(ref_px, 4),
                "level": round(est_px, 4) if order_type == "limit" else None,
                "reason": reason,
                "text": f"[웹훅] {bot['symbol']} {'매수' if side == 'BUY' else '매도'} {qty}주 ({'지정가' if order_type == 'limit' else '시장가'}) — {reason}",
            }],
        }
        st["last_run_date"] = today
        st["last_price"] = ref_px
        bots.log(bot, "approval", f"✋ [웹훅 승인 대기] {bot['symbol']} {'매수' if side == 'BUY' else '매도'} {qty}주 — {reason}")
        bots.save(bot)
        return {"ok": True, "mode": "semi", "staged": 1, "approval_id": app_id, "qty": qty}

    # LIVE execution
    if side == "SELL" and any(p.get("role") == "protection" for p in st["pending"]):
        executor._cancel_protection_conditionals(bot)

    cid = _cid(bot["id"], sig_id)
    if order_type == "limit" and payload_price is not None:
        px = round_price(m, payload_price, "down" if side == "BUY" else "up")
        payload = {"clientOrderId": cid, "symbol": bot["symbol"], "side": side, "orderType": "LIMIT",
                   "timeInForce": "DAY", "quantity": str(qty), "price": fmt_price(m, px)}
        built = {"market": m, "payload": payload, "checks": {"estimated_value": qty * px}}
        r = trading.submit(built, user["id"], source=f"bot:{bot['id']}:webhook")
        sig_record_px = px
    else:
        payload = {"clientOrderId": cid, "symbol": bot["symbol"], "side": side, "orderType": "MARKET",
                   "timeInForce": "DAY", "quantity": str(qty)}
        built = {"market": m, "payload": payload, "checks": {"estimated_value": qty * ref_px}}
        r = trading.submit(built, user["id"], source=f"bot:{bot['id']}:webhook")
        sig_record_px = ref_px

    st["pending"].append({
        "kind": "order",
        "id": r["orderId"],
        "symbol": bot["symbol"],
        "side": side,
        "qty": qty,
        "date": today,
        "synced": 0.0,
        "signal_price": sig_record_px,
    })
    st["last_run_date"] = today
    st["last_price"] = ref_px
    executor._record_day_equity(
        st, today, float(bot["capital"]) + st.get("realized", 0.0) + pos["qty"] * (ref_px - pos["entry_price"])
    )
    bots.log(bot, "order", f"[웹훅 실거래] {bot['symbol']} {'매수' if side == 'BUY' else '매도'} {qty}주 주문 전송 — {reason}")
    protections.evaluate_after_step(bot, st, today)
    bots.save(bot)
    return {"ok": True, "mode": "live", "order_id": r["orderId"], "side": side, "qty": qty}
