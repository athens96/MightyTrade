"""Screener-linked automatic stock rotation bot ('자동 순환매 랭킹 봇') & historical backtester (MT-6).

Periodically runs the Quant Stock Screener on a chosen universe/preset, keeps existing holdings as
long as they remain within `hold_buffer` rank (reducing churn, fees, and sell taxes), sells dropped
symbols first, and buys newly promoted top-ranked stocks in equal weights.
"""
from __future__ import annotations

import hashlib
import math
import time
from datetime import date, datetime, timezone
from typing import Any, Optional

from flask import current_app

from ..account.toss_account import _account, normalize
from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from ..quant import market_timing, portfolio, screener
from ..quant import screener_data as sdata
from ..quant.data import cost_defaults
from ..quant.engine import Bars, Costs
from ..quant.metrics import drawdown_series, performance
from ..trading import orders as trading
from ..users.repository import now_iso
from . import bots, protections

TERMINAL = {"FILLED", "CANCELED", "REJECTED", "REPLACED"}
LIVE_BUY_HEADROOM = 0.005
SELL_SETTLE_POLLS = 5
SELL_SETTLE_INTERVAL = 0.5

ROTATION_CADENCES = {
    "weekly": "매주 초",
    "biweekly": "격주 (2주)",
    "monthly": "매월 초",
}


def _costs(market: Market) -> Costs:
    d = cost_defaults(market)
    return Costs(commission_pct=d["commission_pct"], tax_pct=d["tax_pct"], slippage_pct=d["slippage_pct"])


def _client_order_id(bot: dict, today: str, symbol: str, side: str) -> str:
    return hashlib.sha256(f"{bot['id']}:{today}:rot:{symbol}:{side}".encode()).hexdigest()[:32]


def should_rotate(today: str, last_date: Optional[str], cadence: str) -> Optional[str]:
    if last_date is None:
        return "최초 랭킹 순환매 포트폴리오 구성"
    try:
        dt = date.fromisoformat(today)
        dl = date.fromisoformat(last_date)
    except ValueError:
        return None
    if dt <= dl:
        return None
    if cadence == "weekly":
        if dt.isocalendar()[:2] != dl.isocalendar()[:2]:
            return "주간 정기 랭킹 순환매"
    elif cadence == "biweekly":
        if (dt - dl).days >= 14:
            return "격주 정기 랭킹 순환매"
    else:
        if (dt.year, dt.month) != (dl.year, dl.month):
            return "월간 정기 랭킹 순환매"
    return None


def select_target_picks(
    ranked_items: list[dict[str, Any]],
    held_symbols: list[str],
    top_n: int,
    hold_buffer: int,
) -> tuple[list[dict[str, Any]], list[str]]:
    """Apply rank-buffer retention rule to avoid unnecessary turnover."""
    by_sym = {r["symbol"]: r for r in ranked_items}
    kept: list[dict[str, Any]] = []
    dropped: list[str] = []
    for sym in held_symbols:
        row = by_sym.get(sym)
        if row is not None and row["rank"] <= hold_buffer:
            kept.append(row)
        else:
            dropped.append(sym)
    kept.sort(key=lambda r: r["rank"])
    kept = kept[:top_n]
    kept_syms = {r["symbol"] for r in kept}
    fresh: list[dict[str, Any]] = []
    for r in ranked_items:
        if len(kept) + len(fresh) >= top_n:
            break
        if r["symbol"] not in kept_syms:
            fresh.append(r)
    return kept + fresh, dropped


def _fetch_open_and_close(market: Market, symbols: list[str], today: str, fallback_prices: dict[str, float]) -> tuple[dict[str, float], dict[str, float], bool]:
    provider = get_provider(market)
    start = int(time.time() - 12 * 86400)
    opens: dict[str, float] = {}
    closes: dict[str, float] = {}
    has_all_today = True
    for s in symbols:
        try:
            cs = provider.get_history(market, s, Interval.D1, start)
            tb = next((c for c in cs if datetime.fromtimestamp(c.time, timezone.utc).date().isoformat() == today), None)
            if tb is not None:
                opens[s] = tb.open
                closes[s] = tb.close
            elif cs:
                has_all_today = False
                opens[s] = cs[-1].close
                closes[s] = cs[-1].close
            else:
                has_all_today = False
                px = fallback_prices.get(s, 0.0)
                opens[s] = px
                closes[s] = px
        except Exception:  # noqa: BLE001
            has_all_today = False
            px = fallback_prices.get(s, 0.0)
            opens[s] = px
            closes[s] = px
    return opens, closes, has_all_today


def decide(bot: dict, today: str) -> dict[str, Any]:
    st = bots.state(bot)
    market = Market(bot["market"])
    positions = st.get("positions") or {}
    held_syms = [s for s, p in positions.items() if int(p.get("qty") or 0) > 0]
    top_n = int(bot.get("top_n", 5))
    hold_buffer = max(top_n, int(bot.get("hold_buffer", max(top_n * 2, 10))))
    cadence = str(bot.get("rebalance") or "weekly")
    reason = should_rotate(today, st.get("last_rebalance_date"), cadence)

    scan_limit = max(top_n + 5, hold_buffer + 5, 25)
    screen = screener.run_screen(
        market,
        bot["preset"],
        bot["universe"],
        custom_symbols=bot.get("custom_symbols"),
        strategy_id=bot.get("strategy"),
        exclude_warnings=bool(bot.get("exclude_warnings", True)),
        strict_only=False,
        top_n=min(40, scan_limit),
        user_id=bot.get("user_id"),
    )
    ranked = screen["items"]
    selected, dropped = select_target_picks(ranked, held_syms, top_n, hold_buffer)

    # Apply Market Timing scale if configured
    mt = bot.get("market_timing") or {}
    mt_mode = mt.get("mode") or "off"
    mt_snap = None
    mt_scale = 1.0
    if mt_mode != "off":
        mt_snap = market_timing.current_regime_snapshot(
            bot["market"], mt.get("benchmark"), int(mt.get("ma_short", 20)), int(mt.get("ma_long", 60)), before_date=today
        )
        mt_scale = market_timing.buy_scale_for(mt_mode, mt_snap["regime"])

    all_syms = list(dict.fromkeys(held_syms + [r["symbol"] for r in selected]))
    price_hint = {r["symbol"]: float(r["price"]) for r in ranked}
    for s, p in positions.items():
        price_hint.setdefault(s, float((st.get("last_prices") or {}).get(s) or p.get("entry_price") or 0.0))

    opens, closes, has_today = _fetch_open_and_close(market, all_syms, today, price_hint)
    pos_val = sum(int(positions.get(s, {}).get("qty") or 0) * closes.get(s, 0.0) for s in held_syms)
    total_val = (float(st.get("cash", bot["capital"])) + pos_val) if bot["mode"] == "paper" else max(float(bot["capital"]), pos_val)

    tgt_w: dict[str, float] = {s: 0.0 for s in all_syms}
    if mt_mode == "exit_all" and mt_snap and mt_snap["regime"] == "bear":
        reason = reason or f"마켓 타이밍 현금 대피 ({mt_snap['label']})"
    elif selected:
        slot_w = (1.0 / len(selected)) * mt_scale
        for r in selected:
            tgt_w[r["symbol"]] = slot_w

    cur_w = {s: ((int(positions.get(s, {}).get("qty") or 0) * closes.get(s, 0.0) / total_val) if total_val > 0 else 0.0) for s in all_syms}
    c = _costs(market)
    headroom = max((c.slippage_pct + c.commission_pct) / 100.0, LIVE_BUY_HEADROOM)
    name_map = {r["symbol"]: r["name"] for r in ranked}

    sells, buys = [], []
    for s in all_syms:
        cur_q = int(positions.get(s, {}).get("qty") or 0)
        w = tgt_w.get(s, 0.0)
        px = opens.get(s) or closes.get(s) or 0.0
        tgt_q = math.floor(total_val * w / (px * (1.0 + headroom))) if w > 0 and px > 0 else 0
        diff = tgt_q - cur_q
        nm = name_map.get(s, s)
        if diff < 0:
            tag = "순위 이탈 교체 매도" if w == 0 else "비중 조절 매도"
            sells.append({
                "symbol": s, "name": nm, "side": "sell", "kind": "market", "qty": -diff,
                "price": px, "current_pct": round(cur_w.get(s, 0.0) * 100, 1), "target_pct": round(w * 100, 1),
                "text": f"{nm}({s}) 매도 {-diff}주 — {tag}",
            })
        elif diff > 0:
            buys.append({
                "symbol": s, "name": nm, "side": "buy", "kind": "market", "qty": diff,
                "price": px, "current_pct": round(cur_w.get(s, 0.0) * 100, 1), "target_pct": round(w * 100, 1),
                "text": f"{nm}({s}) 매수 {diff}주 (목표 {w * 100:.1f}%)",
            })

    return {
        "date": today,
        "bar_date": today,
        "due": reason is not None,
        "reason": reason,
        "total_value": total_val,
        "last_prices": closes,
        "open_prices": opens,
        "current_weights": {s: round(cur_w.get(s, 0.0) * 100, 2) for s in all_syms},
        "target_weights": {s: round(tgt_w.get(s, 0.0) * 100, 2) for s in all_syms if tgt_w.get(s, 0.0) > 0},
        "target_weights_raw": tgt_w,
        "selected": [{"symbol": r["symbol"], "name": r["name"], "rank": r["rank"], "score": r["score"]} for r in selected],
        "dropped": dropped,
        "orders": sells + buys,
        "has_today_bars": has_today or len(all_syms) == 0,
        "market_regime": mt_snap,
    }


def preview(bot: dict, today: str) -> dict[str, Any]:
    d = decide(bot, today)
    st = bots.state(bot)
    return {
        "date": d["date"],
        "bar_date": d["bar_date"],
        "due": d["due"],
        "reason": d["reason"],
        "position": {"qty": sum(int(p.get("qty") or 0) for p in (st.get("positions") or {}).values()), "entry_price": 0},
        "cash": st.get("cash", bot["capital"]),
        "today_open": None,
        "last": d["total_value"],
        "current_weights": d["current_weights"],
        "target_weights": d["target_weights"],
        "selected": d["selected"],
        "market_regime": d.get("market_regime"),
        "orders": d["orders"] if d["due"] else [],
        "planned_orders": d["orders"],
    }


def run_paper(bot: dict, today: str) -> dict[str, Any]:
    st = bots.state(bot)
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    d = decide(bot, today)
    if not d["has_today_bars"]:
        raise ApiError(409, "market_not_open", "오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다")

    c = _costs(Market(bot["market"]))
    comm, tax, slip = c.commission_pct / 100.0, c.tax_pct / 100.0, c.slippage_pct / 100.0
    positions = st.setdefault("positions", {})
    buy_block = protections.check_buy_allowed(bot, st, today, current_equity=d["total_value"])

    if d["due"]:
        all_syms = list(d["target_weights_raw"].keys())
        val_open = float(st["cash"]) + sum(int(positions.get(s, {}).get("qty") or 0) * d["open_prices"].get(s, 0.0) for s in all_syms)
        desired = {}
        for s, w in d["target_weights_raw"].items():
            px_b = d["open_prices"].get(s, 0.0) * (1.0 + slip) * (1.0 + comm)
            desired[s] = math.floor(val_open * w / px_b) if w > 0 and px_b > 0 and not buy_block else 0

        n_orders = 0
        for s in list(positions.keys()):
            pos = positions.get(s) or {"qty": 0, "entry_price": 0.0}
            diff = desired.get(s, 0) - int(pos["qty"])
            if diff < 0:
                qty = -diff
                px = d["open_prices"].get(s, pos["entry_price"]) * (1.0 - slip)
                net = qty * px * (1.0 - comm - tax)
                st["cash"] += net
                pos["qty"] = int(pos["qty"]) - qty
                if pos["qty"] <= 0:
                    positions.pop(s, None)
                else:
                    positions[s] = pos
                n_orders += 1
                bots.log(bot, "fill", f"[모의/순환매] {s} 교체 매도 {qty}주 @ {px:,.2f}")

        if not buy_block:
            for s, tgt_q in desired.items():
                pos = positions.get(s) or {"qty": 0, "entry_price": 0.0, "name": s}
                diff = tgt_q - int(pos["qty"])
                if diff > 0:
                    px = d["open_prices"].get(s, 0.0) * (1.0 + slip)
                    unit = px * (1.0 + comm)
                    qty = min(diff, math.floor(st["cash"] / unit)) if unit > 0 else 0
                    if qty > 0:
                        st["cash"] -= qty * unit
                        old_q = int(pos["qty"])
                        new_q = old_q + qty
                        avg = (old_q * float(pos["entry_price"]) + qty * px) / new_q
                        nm = next((x["name"] for x in d["selected"] if x["symbol"] == s), s)
                        positions[s] = {"qty": new_q, "entry_price": avg, "name": nm}
                        n_orders += 1
                        bots.log(bot, "fill", f"[모의/순환매] {nm}({s}) 편입 매수 {qty}주 @ {px:,.2f}")
        else:
            bots.log(bot, "skip", f"[모의/순환매] 신규 매수 보류: {buy_block}", "warn")

        st["last_rebalance_date"] = today
        st["rebalances"].append({
            "date": today, "reason": d["reason"], "trades": n_orders, "weights": d["target_weights"],
        })
        bots.log(bot, "order", f"[모의] {d['reason']} 완료 ({n_orders}건)")
    else:
        bots.log(bot, "decision", f"[모의] 순환매 교체 주기 미도래 (유지)")

    st["last_run_date"] = today
    st["last_prices"] = {**st.get("last_prices", {}), **d["last_prices"]}
    st["target_weights"] = d["target_weights"]
    st["selected_picks"] = d["selected"]
    eq_now = float(st["cash"]) + sum(int(p.get("qty") or 0) * st["last_prices"].get(s, p["entry_price"]) for s, p in positions.items())
    st["equity"].append({"date": today, "value": eq_now})
    protections.evaluate_after_step(bot, st, today)
    bots.save(bot)
    return {"ran": today, "rotated": d["due"]}


def reconcile_live(bot: dict, today: str) -> bool:
    from . import rebalance as rebal
    return rebal.reconcile_live(bot, today)


def run_live(bot: dict, today: str) -> dict[str, Any]:
    from . import rebalance as rebal

    st = bot["state"]["live"]
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    reconcile_live(bot, today)
    d = decide(bot, today)
    if not d["has_today_bars"]:
        raise ApiError(409, "market_not_open", "오늘 시가가 아직 없습니다")

    st["last_run_date"] = today
    bots.save(bot)

    client, seq = _account()
    m = bot["market"]
    currency = "KRW" if m == "KR" else "USD"
    positions = st.setdefault("positions", {})
    all_syms = list(dict.fromkeys(list(positions.keys()) + list(d["target_weights_raw"].keys())))
    live_prices: dict[str, float] = {}
    if all_syms:
        rows = client.request("GET", "/api/v1/prices", params={"symbols": ",".join(all_syms[:30])}) or []
        live_prices = {r["symbol"]: float(r["lastPrice"]) for r in rows if r.get("symbol") and r.get("lastPrice") is not None}
    prices = {s: live_prices.get(s, d["last_prices"].get(s, 0.0)) for s in all_syms}

    c = _costs(Market(m))
    headroom = max((c.slippage_pct + c.commission_pct) / 100.0, LIVE_BUY_HEADROOM)
    buy_block = protections.check_buy_allowed(bot, st, today)

    if d["due"]:
        pos_val = sum(int(positions.get(s, {}).get("qty") or 0) * prices.get(s, 0.0) for s in positions)
        target_cap = max(float(bot["capital"]), pos_val)
        desired = {
            s: (math.floor(target_cap * w / (prices[s] * (1.0 + headroom))) if w > 0 and prices.get(s, 0.0) > 0 and not buy_block else 0)
            for s, w in d["target_weights_raw"].items()
        }
        sent_sells: set[str] = set()
        n_sent = 0
        for s in list(positions.keys()):
            cur_q = int(positions.get(s, {}).get("qty") or 0)
            diff = desired.get(s, 0) - cur_q
            if diff < 0:
                try:
                    sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": s},
                                                     account_seq=seq) or {}).get("sellableQuantity") or 0)
                    qty = int(min(-diff, cur_q, sellable))
                    if qty <= 0:
                        continue
                    r = rebal._send_market(bot, s, "SELL", qty, prices[s], _client_order_id(bot, today, s, "SELL"))
                    st["pending"].append({"kind": "order", "id": r["orderId"], "symbol": s, "side": "SELL",
                                          "qty": qty, "date": today, "synced": 0.0, "signal_price": prices[s]})
                    sent_sells.add(r["orderId"])
                    n_sent += 1
                    bots.log(bot, "order", f"[순환매] {s} 교체 매도 시장가 {qty}주")
                except ApiError as e:
                    bots.log(bot, "error", f"[순환매] {s} 매도 실패: {e.message}", "error", {"code": e.code})

        has_buys = not buy_block and any(desired.get(s, 0) > int(positions.get(s, {}).get("qty") or 0) for s in desired)
        if sent_sells and has_buys:
            rebal._wait_sell_fills(bot, today, sent_sells)

        if has_buys:
            bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
            avail_cash = float(bp.get("cashBuyingPower") or 0.0)
            invested = sum(int(positions.get(s, {}).get("qty") or 0) * prices.get(s, 0.0) for s in positions)
            budget = min(avail_cash, max(float(bot["capital"]) - invested, 0.0))
            for s, tgt_q in desired.items():
                cur_q = int(positions.get(s, {}).get("qty") or 0)
                diff = tgt_q - cur_q
                if diff <= 0:
                    continue
                unit = prices[s] * (1.0 + headroom)
                qty = min(diff, math.floor(budget / unit)) if unit > 0 else 0
                if qty <= 0:
                    continue
                try:
                    r = rebal._send_market(bot, s, "BUY", qty, prices[s], _client_order_id(bot, today, s, "BUY"))
                    st["pending"].append({"kind": "order", "id": r["orderId"], "symbol": s, "side": "BUY",
                                          "qty": qty, "date": today, "synced": 0.0, "signal_price": prices[s]})
                    budget = max(budget - qty * unit, 0.0)
                    n_sent += 1
                    bots.log(bot, "order", f"[순환매] {s} 편입 매수 시장가 {qty}주")
                except ApiError as e:
                    bots.log(bot, "error", f"[순환매] {s} 매수 실패: {e.message}", "error", {"code": e.code})

        st["last_rebalance_date"] = today
        st["rebalances"].append({
            "date": today, "reason": d["reason"], "trades": n_sent, "weights": d["target_weights"],
        })

    st["last_prices"] = {**st.get("last_prices", {}), **prices}
    st["target_weights"] = d["target_weights"]
    st["selected_picks"] = d["selected"]
    unreal = sum(int(p.get("qty") or 0) * (prices.get(s, p["entry_price"]) - p["entry_price"]) for s, p in positions.items())
    st["equity"].append({"date": today, "value": float(bot["capital"]) + st.get("realized", 0.0) + unreal})
    protections.evaluate_after_step(bot, st, today)
    bots.save(bot)
    return {"ran": today, "rotated": d["due"]}


def run(bot: dict, today: str) -> dict[str, Any]:
    return run_paper(bot, today) if bot["mode"] == "paper" else run_live(bot, today)


def run_rotation_backtest(
    market: Market,
    preset_id: str,
    universe_id: str,
    *,
    custom_symbols: Optional[list[str]] = None,
    top_n: int = 5,
    hold_buffer: int = 10,
    cadence: str = "monthly",
    period: str = "3y",
    capital: float = 10_000_000,
    costs: Optional[Costs] = None,
    user_id: Optional[str] = None,
) -> dict[str, Any]:
    """Historical zero-lookahead backtest of periodic top-N stock rotation with rank buffer."""
    if cadence not in ROTATION_CADENCES:
        raise ApiError(400, "invalid_cadence", "순환 주기는 weekly / biweekly / monthly 중 하나여야 합니다")
    preset = screener.SCREENER_PRESETS.get(preset_id)
    if not preset:
        raise ApiError(400, "unknown_preset")
    is_mock = current_app.config["MARKET_PROVIDERS"][market.value] != "toss"
    uni_norm, candidates = screener._resolve_candidates(market, universe_id, custom_symbols, is_mock=is_mock, user_id=user_id)
    # Cap universe to 40 symbols for fast multi-year historical rotation simulation
    candidates = candidates[:40]
    syms = [s for s, _ in candidates]
    name_map = dict(candidates)

    days_map = {"1y": 365, "3y": 365 * 3, "5y": 365 * 5}
    days = days_map.get(period, 365 * 3)
    start_ts = int(time.time() - days * 86400)
    fetch_from = int(start_ts - 320 * 86400)

    bars_by_sym: dict[str, Bars] = {}
    if not is_mock:
        bars_by_sym = sdata.fetch_public_history(market.value, syms, period=period if period in ("1y", "3y", "5y") else "3y")
    missing = [s for s in syms if s not in bars_by_sym]
    if missing:
        provider = get_provider(market)
        for sym in missing:
            try:
                cs = provider.get_history(market, sym, Interval.D1, fetch_from)
                if len(cs) >= 60:
                    bars_by_sym[sym] = Bars.from_candles(cs)
            except Exception:  # noqa: BLE001
                continue

    if len(bars_by_sym) < 2:
        raise ApiError(400, "not_enough_data", "순환매 백테스트에 필요한 시세 데이터가 부족합니다")

    times, opens, closes = portfolio.align_bars(bars_by_sym)
    valid_syms = list(closes.keys())
    warmup = min(200, max(30, len(times) // 4))
    first = next((i for i, t in enumerate(times) if t >= start_ts), warmup)
    start_idx = max(first, warmup)
    if start_idx >= len(times) - 2:
        start_idx = max(20, len(times) // 3)
    if start_idx >= len(times) - 2:
        raise ApiError(400, "not_enough_data", "공통 거래일 구간이 너무 짧습니다")

    c = costs or _costs(market)
    comm, tax, slip = c.commission_pct / 100.0, c.tax_pct / 100.0, c.slippage_pct / 100.0
    cash = float(capital)
    holdings: dict[str, int] = {}
    last_rot_day: Optional[str] = None
    planned_weights: Optional[dict[str, float]] = None
    planned_reason = ""
    rebalances: list[dict[str, Any]] = []
    equity: list[float] = []

    for j in range(start_idx, len(times)):
        day_str = portfolio._day_str(times[j])
        if planned_weights is not None:
            val_open = cash + sum(holdings.get(s, 0) * opens[s][j] for s in valid_syms)
            desired = {}
            for s, w in planned_weights.items():
                px_b = opens[s][j] * (1.0 + slip) * (1.0 + comm)
                desired[s] = math.floor(val_open * w / px_b) if w > 0 and px_b > 0 else 0
            traded_val = 0.0
            n_trades = 0
            for s in list(holdings.keys()):
                diff = desired.get(s, 0) - holdings.get(s, 0)
                if diff < 0:
                    q = -diff
                    px = opens[s][j] * (1.0 - slip)
                    cash += q * px * (1.0 - comm - tax)
                    holdings[s] = holdings[s] - q
                    if holdings[s] <= 0:
                        holdings.pop(s, None)
                    traded_val += q * px
                    n_trades += 1
            for s, tgt_q in desired.items():
                diff = tgt_q - holdings.get(s, 0)
                if diff > 0:
                    px = opens[s][j] * (1.0 + slip)
                    unit = px * (1.0 + comm)
                    q = min(diff, math.floor(cash / unit)) if unit > 0 else 0
                    if q > 0:
                        cash -= q * unit
                        holdings[s] = holdings.get(s, 0) + q
                        traded_val += q * px
                        n_trades += 1
            rebalances.append({
                "time": times[j],
                "date": day_str,
                "reason": planned_reason,
                "trades": n_trades,
                "turnover_pct": round((traded_val / val_open * 100.0) if val_open > 0 else 0.0, 2),
                "weights": {s: round(w * 100.0, 1) for s, w in planned_weights.items() if w > 0},
            })
            last_rot_day = day_str
            planned_weights = None

        eq = cash + sum(holdings.get(s, 0) * closes[s][j] for s in holdings)
        equity.append(eq)

        if j < len(times) - 1:
            next_day = portfolio._day_str(times[j + 1])
            due_reason = should_rotate(next_day, last_rot_day, cadence)
            if due_reason:
                # Score historical slice [0 .. j] with zero look-ahead
                slice_rows = []
                for s in valid_syms:
                    cs_slice = closes[s][: j + 1]
                    if len(cs_slice) < 25 or cs_slice[-1] <= 0:
                        continue
                    b_slice = Bars(times[: j + 1], opens[s][: j + 1], cs_slice, cs_slice, cs_slice, [1000] * (j + 1))
                    st_s = screener._compute_price_stats(b_slice)
                    slice_rows.append({
                        "symbol": s, "name": name_map.get(s, s), "price": st_s["price"],
                        "change_1d_pct": st_s["change_1d_pct"], "ret_1m_pct": st_s["ret_1m_pct"],
                        "ret_3m_pct": st_s["ret_3m_pct"], "ret_6m_pct": st_s["ret_6m_pct"],
                        "ret_12m_pct": st_s["ret_12m_pct"], "vol_60d_pct": st_s["vol_60d_pct"],
                        "rsi2": st_s["rsi2"], "ibs": st_s["ibs"], "above_ma200": st_s["above_ma200"],
                        "near_52w_pct": st_s["near_52w_pct"], "vol_ratio_20d": None,
                        "per": 12.0, "pbr": 1.2, "roe": 15.0, "dividend_yield": 3.0, "market_cap": None,
                        "flow_5d_for": 100, "flow_5d_inst": 100, "flow_20d_for": 200, "flow_20d_inst": 200,
                        "foreign_delta_pct": 0.1, "fit_return_pct": st_s["ret_6m_pct"], "fit_cagr_pct": st_s["ret_6m_pct"],
                        "fit_mdd_pct": -10.0, "fit_sharpe": 1.0, "fit_win_rate": 55.0, "fit_trades": 5,
                        "warnings": [], "matched": False, "match_label": "", "score": 0.0, "reasons": [],
                    })
                eff_preset = preset_id if preset_id in ("momentum_quality", "oversold_bounce") else "momentum_quality"
                scored = screener._score_and_explain(eff_preset, slice_rows)
                picks, _ = select_target_picks(scored, list(holdings.keys()), top_n, max(top_n, hold_buffer))
                if picks:
                    w_each = 1.0 / len(picks)
                    planned_weights = {p["symbol"]: w_each for p in picks}
                    planned_reason = due_reason

    eval_times = times[start_idx:]
    # Equal-weight buy-and-hold benchmark across candidate universe
    bench_eq: list[float] = []
    per_sym_cap = float(capital) / len(valid_syms)
    b_shares = {s: per_sym_cap / closes[s][start_idx] for s in valid_syms if closes[s][start_idx] > 0}
    for j in range(start_idx, len(times)):
        bench_eq.append(sum(q * closes[s][j] for s, q in b_shares.items()))

    final_eq = equity[-1] if equity else float(capital)
    cur_weights = {
        s: round((q * closes[s][-1] / final_eq) * 100.0, 2)
        for s, q in holdings.items()
        if final_eq > 0
    }
    return {
        "market": market.value,
        "preset": preset_id,
        "universe": uni_norm,
        "top_n": top_n,
        "hold_buffer": max(top_n, hold_buffer),
        "cadence": cadence,
        "capital": capital,
        "currency": "KRW" if market == Market.KR else "USD",
        "period": {"start": eval_times[0], "end": eval_times[-1]},
        "equity": [
            {"time": t, "equity": round(e, 4), "benchmark": round(be, 4), "drawdown": round(dd * 100, 4)}
            for t, e, be, dd in zip(eval_times, equity, bench_eq, drawdown_series(equity))
        ],
        "metrics": {**performance(eval_times, equity), "rebalances": len(rebalances)},
        "benchmark": performance(eval_times, bench_eq),
        "rebalances": rebalances,
        "current_weights": cur_weights,
    }
