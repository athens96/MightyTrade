"""Semi-automated '1-Click Confirm-to-Trade' ('승인 후 주문') staging & execution (MT-8).

In `semi` mode, the scheduler computes exact signals & live broker sizing, stages a time-bounded
proposal (`pending_approval`) on the bot's live state, and notifies the user via Telegram/Discord/Slack.
Nothing is sent to Toss until the user clicks Approve (within the session expiry window) with an
unlocked trading session or fresh OTP.
"""
from __future__ import annotations

import hashlib
import math
import uuid
from datetime import datetime, timezone
from typing import Any, Optional

from ..account.toss_account import _account, normalize
from ..errors import ApiError
from ..market.models import Market
from ..quant.data import cost_defaults
from ..trading import orders as trading
from ..trading.ticks import fmt_price, round_price
from ..users.repository import now_iso
from . import bots, executor, protections

LIVE_BUY_HEADROOM = 0.005


def _cid(bot_id: str, approval_id: str, idx: int) -> str:
    return hashlib.sha256(f"{bot_id}:semi:{approval_id}:{idx}".encode()).hexdigest()[:32]


def is_expired(proposal: Optional[dict], now: Optional[datetime] = None) -> bool:
    if not proposal:
        return True
    exp = proposal.get("expires_at")
    if not exp:
        return False
    now_dt = now or datetime.now(timezone.utc)
    try:
        return now_dt >= datetime.fromisoformat(exp)
    except ValueError:
        return False


def active_proposal(st: dict, now: Optional[datetime] = None) -> Optional[dict]:
    prop = st.get("pending_approval")
    if not prop or is_expired(prop, now):
        return None
    return prop


def stage_from_scheduler(bot: dict, today: str, session_end: datetime, now: Optional[datetime] = None) -> dict[str, Any]:
    """Compute today's orders for a `semi` bot and stage `pending_approval` without placing Toss orders."""
    st = bots.state(bot)
    is_intra = executor.is_intraday(bot)
    if not is_intra and st.get("last_run_date") == today:
        return {"skipped": "already_staged"}

    executor.reconcile_live(bot, today)
    pv = executor.preview(bot, today)
    if is_intra:
        loaded = executor.load_intraday(bot)
        bar_time = loaded[1].time[-1]
        if st.get("last_bar_time") == bar_time:
            return {"skipped": "already_staged_bar"}
        st["last_bar_time"] = bar_time

    st["last_run_date"] = today
    raw_orders = pv.get("orders") or []
    if not raw_orders:
        bots.log(bot, "decision", f"[반자동] {pv.get('bar_date', today)} 기준 주문 신호 없음")
        bots.save(bot)
        return {"staged": 0}

    buy_block = protections.check_buy_allowed(bot, st, today)
    client, seq = _account()
    m = bot["market"]
    currency = "KRW" if m == "KR" else "USD"
    d_cost = cost_defaults(Market(m))
    headroom = max((d_cost["slippage_pct"] + d_cost["commission_pct"]) / 100.0, LIVE_BUY_HEADROOM)

    bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
    avail_cash = float(bp.get("cashBuyingPower") or 0.0)
    kind = bots.bot_kind(bot)
    if kind in ("rebalance", "rotation"):
        invested = sum(int(p.get("qty") or 0) * float(p.get("entry_price") or 0.0) for p in (st.get("positions") or {}).values())
    else:
        invested = float(st["position"]["qty"]) * float(st["position"]["entry_price"])
    rem_budget = min(avail_cash, max(float(bot["capital"]) - invested, 0.0))

    staged: list[dict[str, Any]] = []
    for o in raw_orders:
        side = str(o.get("side", "")).upper()
        if side == "BUY" and buy_block:
            bots.log(bot, "skip", f"[반자동] 신규 매수 보류: {buy_block}", "warn")
            continue
        sym = str(o.get("symbol") or bot["symbol"]).strip().upper()
        p_rows = client.request("GET", "/api/v1/prices", params={"symbols": sym}) or []
        cur_px = float(p_rows[0]["lastPrice"]) if p_rows and p_rows[0].get("lastPrice") is not None else float(o.get("price") or pv.get("last") or 0.0)
        o_kind = o.get("kind") or "market"
        level = o.get("limit") if o_kind == "limit" else (
            o.get("stop") if o.get("stop") is not None else (
                (pv.get("today_open") or cur_px) + float(o["stop_from_open"]) if o.get("stop_from_open") is not None else None
            )
        )

        if side == "BUY":
            ref_px = float(level or cur_px)
            if ref_px <= 0:
                continue
            max_q = math.floor(rem_budget / (ref_px * (1.0 + headroom)))
            req_q = int(o["qty"]) if o.get("qty") is not None else max_q
            qty = min(req_q, max_q)
            if qty <= 0:
                continue
            rem_budget = max(rem_budget - qty * ref_px * (1.0 + headroom), 0.0)
        else:
            sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": sym},
                                             account_seq=seq) or {}).get("sellableQuantity") or 0)
            held_q = (
                int(((st.get("positions") or {}).get(sym) or {}).get("qty") or 0)
                if kind in ("rebalance", "rotation")
                else int(st["position"]["qty"])
            )
            req_q = int(o["qty"]) if o.get("qty") is not None else held_q
            qty = int(min(req_q, held_q, sellable))
            if qty <= 0:
                continue

        staged.append({
            "symbol": sym,
            "side": side,
            "kind": o_kind,
            "qty": qty,
            "signal_price": round(cur_px, 4),
            "level": round(float(level), 4) if level is not None else None,
            "reason": o.get("reason") or "",
            "text": o.get("text") or f"{sym} {'매수' if side == 'BUY' else '매도'} {qty}주",
        })

    if not staged:
        bots.log(bot, "skip", "[반자동] 실행 가능한 주문 수량이 없어 승인 대기를 생략했습니다", "info")
        bots.save(bot)
        return {"staged": 0}

    app_id = uuid.uuid4().hex[:12]
    proposal = {
        "id": app_id,
        "date": today,
        "created_at": (now or datetime.now(timezone.utc)).isoformat(),
        "expires_at": session_end.isoformat(),
        "reason": pv.get("reason") or "",
        "orders": staged,
    }
    st["pending_approval"] = proposal
    summary = " / ".join(f"{x['symbol']} {'매수' if x['side'] == 'BUY' else '매도'} {x['qty']}주" for x in staged[:3])
    if len(staged) > 3:
        summary += f" 외 {len(staged) - 3}건"
    bots.log(bot, "approval", f"✋ [승인 대기] {summary} — 화면에서 [1클릭 승인] 시 실제 주문 전송")
    bots.save(bot)
    return {"staged": len(staged), "approval_id": app_id}


def approve_proposal(bot: dict, now: Optional[datetime] = None) -> dict[str, Any]:
    """Send staged `pending_approval` orders to Toss with idempotent clientOrderIds."""
    st = bots.state(bot)
    prop = st.get("pending_approval")
    if not prop:
        raise ApiError(404, "no_pending_approval", "승인 대기 중인 주문이 없습니다")
    if is_expired(prop, now):
        st.pop("pending_approval", None)
        bots.save(bot)
        raise ApiError(410, "approval_expired", "승인 가능 시간(정규장 마감)이 지나 자동 만료되었습니다")

    # Clear proposal BEFORE sending orders so a network failure cannot double-approve
    st.pop("pending_approval", None)
    bots.save(bot)

    m = bot["market"]
    today = prop["date"]
    kind = bots.bot_kind(bot)
    sent = 0

    for idx, item in enumerate(prop["orders"]):
        sym = item["symbol"]
        side = item["side"]
        qty = int(item["qty"])
        sig_px = float(item.get("signal_price") or 0.0)
        o_kind = item.get("kind") or "market"
        level = item.get("level")
        cid = _cid(bot["id"], prop["id"], idx)

        try:
            if o_kind == "limit" and level is not None:
                px = round_price(m, float(level), "down" if side == "BUY" else "up")
                payload = {"clientOrderId": cid, "symbol": sym, "side": side, "orderType": "LIMIT",
                           "timeInForce": "DAY", "quantity": str(qty), "price": fmt_price(m, px)}
                built = {"market": m, "payload": payload, "checks": {"estimated_value": qty * px}}
                r = trading.submit(built, bot["user_id"], source=f"bot:{bot['id']}")
                st["pending"].append({
                    "kind": "order", "id": r["orderId"], "symbol": sym, "side": side,
                    "qty": qty, "date": today, "synced": 0.0, "signal_price": px,
                })
            elif o_kind == "stop" and level is not None:
                r = executor._send_conditional(bot, side, qty, float(level), today, cid)
                st["pending"].append({
                    "kind": "conditional", "id": r["conditionalOrderId"], "symbol": sym, "side": side,
                    "qty": qty, "date": today, "synced": 0.0, "signal_price": float(level),
                })
            else:
                payload = {"clientOrderId": cid, "symbol": sym, "side": side, "orderType": "MARKET",
                           "timeInForce": "DAY", "quantity": str(qty)}
                built = {"market": m, "payload": payload, "checks": {"estimated_value": qty * max(sig_px, 1.0)}}
                r = trading.submit(built, bot["user_id"], source=f"bot:{bot['id']}")
                st["pending"].append({
                    "kind": "order", "id": r["orderId"], "symbol": sym, "side": side,
                    "qty": qty, "date": today, "synced": 0.0, "signal_price": sig_px,
                })
            sent += 1
            bots.log(bot, "order", f"[반자동 승인 전송] {sym} {'매수' if side == 'BUY' else '매도'} {qty}주")
        except ApiError as e:
            bots.log(bot, "error", f"[반자동 승인 실패] {sym}: {e.message}", "error", {"code": e.code})

    if sent > 0:
        if kind in ("rebalance", "rotation"):
            st["last_rebalance_date"] = today
            st.setdefault("rebalances", []).append({
                "date": today, "reason": prop.get("reason") or "반자동 승인 리밸런싱", "trades": sent, "weights": st.get("target_weights") or {},
            })
        elif kind == "dca":
            st["last_dca_date"] = today
    bots.save(bot)
    return {"sent": sent}


def skip_proposal(bot: dict) -> None:
    st = bots.state(bot)
    if not st.pop("pending_approval", None):
        raise ApiError(404, "no_pending_approval", "건너뛸 승인 대기 주문이 없습니다")
    bots.log(bot, "skip", "[반자동] 사용자가 오늘 승인 대기 주문을 건너뛰었습니다", "info")
    bots.save(bot)
