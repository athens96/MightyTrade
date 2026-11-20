"""One trading-day step for a bot.

Timing mirrors the backtester (app/quant/engine.py): the strategy decides on COMPLETED daily bars only
(up to yesterday), and today's actions happen after the regular session opens:
- market order            -> paper: fill at today's open | live: MARKET order right away
- stop order (breakout)   -> paper: settled at the next run against today's high/low
                             live: Toss conditional order (SINGLE, MARKET, expires today);
                                   if the price already crossed the level -> MARKET order now
Live idempotency: `last_run_date` is saved BEFORE any order is sent (a crash can skip a day, never
double-order), and clientOrderIds are derived from (bot, date, order#) so Toss dedupes retries.
"""
from __future__ import annotations

import bisect
import hashlib
import math
import time
from datetime import datetime, timezone
from typing import Any, Optional

from ..account.toss_account import _account, normalize
from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from ..quant.data import cost_defaults
from ..quant.engine import Bars, Context, Costs, Order, Position
from ..quant.strategies import STRATEGIES
from ..storage import get_store
from ..trading import orders as trading
from ..trading.ticks import fmt_price, round_price
from ..users.repository import now_iso
from . import bots

TERMINAL = {"FILLED", "CANCELED", "REJECTED", "REPLACED"}
LIVE_BUY_HEADROOM = 0.005


def day_of(t: int) -> str:
    return datetime.fromtimestamp(t, timezone.utc).date().isoformat()


# ------------------------------------------------------------------ decide (pure)
def load(bot: dict, today: str) -> tuple:
    cls = STRATEGIES[bot["strategy"]]
    strat = cls(bot["params"])
    market = Market(bot["market"])
    start = int(time.time() - (strat.warmup() * 1.6 + 40) * 86400)
    candles = get_provider(market).get_history(market, bot["symbol"], Interval.D1, start)
    past = [c for c in candles if day_of(c.time) < today]
    today_bar = next((c for c in candles if day_of(c.time) == today), None)
    if len(past) < strat.warmup() + 2:
        raise ApiError(400, "not_enough_data", "전략 계산에 필요한 과거 데이터가 부족합니다")
    return candles, Bars.from_candles(past), today_bar, strat


def decide(bot: dict, today: str, loaded: Optional[tuple] = None) -> dict:
    """What the strategy wants to do today, based on bars up to yesterday. No side effects."""
    candles, bars, today_bar, strat = loaded or load(bot, today)
    st = bots.state(bot)
    pos = st["position"]
    entry_index = -1
    if pos["qty"] > 0:
        days = [day_of(t) for t in bars.time]
        entry_index = min(bisect.bisect_left(days, pos.get("entry_date") or days[-1]), len(days) - 1)
    position = Position(pos["qty"], pos["entry_price"], entry_index)
    last = today_bar.close if today_bar else bars.close[-1]
    cash = st["cash"] if bot["mode"] == "paper" else max(bot["capital"] - pos["qty"] * pos["entry_price"], 0.0)
    strat.prepare(bars)
    planned = strat.on_bar(len(bars) - 1, Context(position, cash, cash + pos["qty"] * last)) or []
    return {"date": today, "bar_date": day_of(bars.time[-1]), "orders": planned, "today_bar": today_bar,
            "candles": candles, "position": pos, "cash": cash, "last": last}


def describe(o: Order, today_open: Optional[float]) -> str:
    side = "매수" if o.side == "buy" else "매도"
    if o.kind == "market":
        return f"{side} (시장가) — {o.reason}"
    if o.stop is not None:
        return f"{side} 조건: {'≥' if o.side == 'buy' else '≤'} {o.stop:,.2f} — {o.reason}"
    level = f"{today_open + (o.stop_from_open or 0):,.2f}" if today_open else f"시가 + {o.stop_from_open:,.2f}"
    return f"{side} 조건: ≥ {level} (시가 + {o.stop_from_open:,.2f}) — {o.reason}"


def preview(bot: dict, today: str) -> dict:
    d = decide(bot, today)
    tb = d["today_bar"]
    return {
        "date": today, "bar_date": d["bar_date"], "position": d["position"], "cash": d["cash"],
        "today_open": tb.open if tb else None, "last": d["last"],
        "orders": [{"side": o.side, "kind": o.kind, "stop": o.stop, "stop_from_open": o.stop_from_open, "qty": o.qty,
                    "reason": o.reason, "text": describe(o, tb.open if tb else None)} for o in d["orders"]],
    }


def _level(o: Order, today_open: float) -> float:
    return o.stop if o.stop is not None else today_open + (o.stop_from_open or 0.0)


def _costs(market: Market) -> Costs:
    d = cost_defaults(market)
    return Costs(commission_pct=d["commission_pct"], tax_pct=d["tax_pct"], slippage_pct=d["slippage_pct"])


# ------------------------------------------------------------------ paper
def _paper_buy(st: dict, qty: int, price: float, c: Costs, date: str, reason: str) -> None:
    px = price * (1 + c.slippage_pct / 100)
    value = qty * px
    fee = value * c.commission_pct / 100
    st["cash"] -= value + fee
    st["position"] = {"qty": qty, "entry_price": px, "entry_date": date}
    st["trades"].append({"entry_date": date, "entry_price": px, "qty": qty, "entry_reason": reason, "cost_basis": value + fee,
                         "exit_date": None, "exit_price": None, "exit_reason": "", "pnl": None, "return_pct": None})


def _paper_sell(st: dict, price: float, c: Costs, date: str, reason: str) -> None:
    pos = st["position"]
    px = price * (1 - c.slippage_pct / 100)
    value = pos["qty"] * px
    net = value - value * (c.commission_pct + c.tax_pct) / 100
    st["cash"] += net
    t = next((t for t in reversed(st["trades"]) if t["exit_date"] is None), None)
    if t:
        t.update(exit_date=date, exit_price=px, exit_reason=reason, pnl=net - t["cost_basis"],
                 return_pct=(net / t["cost_basis"] - 1) * 100)
    st["position"] = {"qty": 0, "entry_price": 0.0, "entry_date": None}


def _paper_qty(st: dict, o: Order, price: float, c: Costs) -> int:
    qty = math.floor(st["cash"] / (price * (1 + c.slippage_pct / 100) * (1 + c.commission_pct / 100)))
    return min(qty, int(o.qty)) if o.qty is not None else qty


def settle_paper(bot: dict, candles: list, today: str, c: Costs) -> None:
    """Resolve yesterday's stop orders against that day's bar (same rule as the backtester)."""
    st = bots.state(bot)
    by_day = {day_of(x.time): x for x in candles}
    keep = []
    for p in st["pending"]:
        if p["active_date"] >= today:
            keep.append(p)
            continue
        bar = by_day.get(p["active_date"])
        if bar is None:
            continue
        level, holding = p["level"], st["position"]["qty"] > 0
        if p["side"] == "buy" and not holding and bar.high >= level:
            fill = max(bar.open, level)
            qty = _paper_qty(st, Order("buy", qty=p.get("qty")), fill, c)
            if qty > 0:
                _paper_buy(st, qty, fill, c, p["active_date"], p["reason"])
                bots.log(bot, "fill", f"[모의] 매수 체결 {qty}주 @ {fill:,.2f} ({p['active_date']} 조건 충족)")
        elif p["side"] == "sell" and holding and bar.low <= level:
            fill = min(bar.open, level)
            _paper_sell(st, fill, c, p["active_date"], p["reason"])
            bots.log(bot, "fill", f"[모의] 매도 체결 @ {fill:,.2f} ({p['active_date']} 조건 충족)")
    st["pending"] = keep


def run_paper(bot: dict, today: str) -> dict:
    st = bots.state(bot)
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    market = Market(bot["market"])
    c = _costs(market)
    loaded = load(bot, today)
    candles, _, today_bar, _ = loaded
    if today_bar is None:
        raise ApiError(409, "market_not_open", "오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다")
    settle_paper(bot, candles, today, c)
    d = decide(bot, today, loaded)
    o_open = today_bar.open
    for o in d["orders"]:
        holding = st["position"]["qty"] > 0
        if (o.side == "buy" and holding) or (o.side == "sell" and not holding):
            continue
        if o.kind == "market":
            if o.side == "buy":
                qty = _paper_qty(st, o, o_open, c)
                if qty <= 0:
                    bots.log(bot, "skip", "[모의] 자금 부족으로 매수하지 못했습니다", "warn")
                    continue
                _paper_buy(st, qty, o_open, c, today, o.reason)
                bots.log(bot, "fill", f"[모의] 매수 체결 {qty}주 @ 시가 {o_open:,.2f} — {o.reason}")
            else:
                _paper_sell(st, o_open, c, today, o.reason)
                bots.log(bot, "fill", f"[모의] 매도 체결 @ 시가 {o_open:,.2f} — {o.reason}")
        else:
            level = _level(o, o_open)
            gapped = (o.side == "buy" and o_open >= level) or (o.side == "sell" and o_open <= level)
            if gapped:
                if o.side == "buy":
                    qty = _paper_qty(st, o, o_open, c)
                    if qty > 0:
                        _paper_buy(st, qty, o_open, c, today, o.reason)
                        bots.log(bot, "fill", f"[모의] 시가가 기준가를 넘어 시가 {o_open:,.2f}에 매수 {qty}주 — {o.reason}")
                else:
                    _paper_sell(st, o_open, c, today, o.reason)
                    bots.log(bot, "fill", f"[모의] 시가가 기준가 아래라 시가 {o_open:,.2f}에 매도 — {o.reason}")
            else:
                st["pending"].append({"active_date": today, "side": o.side, "level": level, "qty": o.qty, "reason": o.reason})
                bots.log(bot, "order", f"[모의] {describe(o, o_open)} (오늘 하루 유효)")
    if not d["orders"]:
        bots.log(bot, "decision", f"[모의] {d['bar_date']} 종가 기준 신호 없음")
    st["last_run_date"] = today
    st["last_price"] = today_bar.close
    pos = st["position"]
    st["equity"].append({"date": today, "value": st["cash"] + pos["qty"] * today_bar.close})
    bots.save(bot)
    return {"ran": today}


# ------------------------------------------------------------------ live
def _client_order_id(bot: dict, today: str, n: int, tag: str) -> str:
    return hashlib.sha256(f"{bot['id']}:{today}:{n}:{tag}".encode()).hexdigest()[:32]


def _apply_live_fill(st: dict, side: str, qty: float, avg: float, date: str) -> None:
    pos = st["position"]
    if side == "BUY":
        new_qty = pos["qty"] + qty
        pos["entry_price"] = (pos["qty"] * pos["entry_price"] + qty * avg) / new_qty
        if pos["qty"] == 0:
            pos["entry_date"] = date
            st["trades"].append({"entry_date": date, "entry_price": avg, "qty": qty, "entry_reason": "",
                                 "exit_date": None, "exit_price": None, "exit_reason": "", "pnl": None, "return_pct": None})
        pos["qty"] = new_qty
    else:
        sold = min(qty, pos["qty"])
        pnl = (avg - pos["entry_price"]) * sold
        st["realized"] = st.get("realized", 0.0) + pnl
        pos["qty"] -= sold
        t = next((t for t in reversed(st["trades"]) if t["exit_date"] is None), None)
        if t and pos["qty"] == 0:
            t.update(exit_date=date, exit_price=avg, pnl=(avg - t["entry_price"]) * t["qty"],
                     return_pct=(avg / t["entry_price"] - 1) * 100)
        if pos["qty"] == 0:
            pos.update(entry_price=0.0, entry_date=None)


def reconcile_live(bot: dict, today: str) -> bool:
    """Pull fills of the bot's own orders from Toss into its position. Returns True if anything changed."""
    st = bot["state"]["live"]
    if not st["pending"]:
        return False
    client, seq = _account()
    changed, keep = False, []
    for p in st["pending"]:
        try:
            if p["kind"] == "conditional":
                co = normalize(client.request("GET", f"/api/v1/conditional-orders/{p['id']}", account_seq=seq) or {})
                trig = (co.get("first") or {}).get("triggeredOrderId")
                if trig:
                    p.update(kind="order", id=trig)
                    bots.log(bot, "fill", f"조건 충족 → 주문 생성 ({p['side']})")
                elif co.get("status") in ("COMPLETED", "EXPIRED") or (co.get("first") or {}).get("status") in ("CANCELED", "EXPIRED"):
                    changed = True
                    continue
                else:
                    keep.append(p)
                    continue
            o = normalize(client.request("GET", f"/api/v1/orders/{p['id']}", account_seq=seq) or {})
            ex = o.get("execution") or {}
            filled = ex.get("filledQuantity") or 0.0
            delta = filled - p.get("synced", 0.0)
            if delta > 0 and ex.get("averageFilledPrice"):
                _apply_live_fill(st, p["side"], delta, ex["averageFilledPrice"], today)
                p["synced"] = filled
                changed = True
                bots.log(bot, "fill", f"{'매수' if p['side'] == 'BUY' else '매도'} 체결 {delta:g}주 @ {ex['averageFilledPrice']:,.2f}")
            if o.get("status") in TERMINAL:
                if o.get("status") == "REJECTED":
                    bots.log(bot, "error", f"주문 거부됨 ({p['side']})", "error")
                changed = True
                continue
            keep.append(p)
        except ApiError as e:
            bots.log(bot, "error", f"체결 확인 실패: {e.message}", "warn")
            keep.append(p)
    st["pending"] = keep
    st["last_reconcile"] = now_iso()
    return changed


def _send_market(bot: dict, side: str, qty: int, est_price: float, cid: str) -> dict:
    payload = {"clientOrderId": cid, "symbol": bot["symbol"], "side": side, "orderType": "MARKET",
               "timeInForce": "DAY", "quantity": str(qty)}
    built = {"market": bot["market"], "payload": payload, "checks": {"estimated_value": qty * est_price}}
    return trading.submit(built, bot["user_id"], source=f"bot:{bot['id']}")


def _send_conditional(bot: dict, side: str, qty: int, level: float, today: str, cid: str) -> dict:
    m = bot["market"]
    trading.check_limits(bot["user_id"], Market(m), qty * level)
    trigger = round_price(m, level, "up" if side == "BUY" else "down")
    payload = {"clientOrderId": cid, "symbol": bot["symbol"], "type": "SINGLE", "quantity": str(qty), "orderType": "MARKET",
               "expireDate": today, "first": {"orderSide": side, "triggerPrice": fmt_price(m, trigger)}}
    client, seq = _account()
    result = normalize(client.request("POST", "/api/v1/conditional-orders", json=payload, account_seq=seq) or {})
    get_store().put("orders", cid, {"user_id": bot["user_id"], "kind": "conditional_single", "source": f"bot:{bot['id']}",
                                    "market": m, "symbol": bot["symbol"], "side": side, "payload": payload, "result": result,
                                    "date": trading.kst_today(), "created_at": now_iso()})
    return result


def run_live(bot: dict, today: str) -> dict:
    st = bot["state"]["live"]
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    reconcile_live(bot, today)
    loaded = load(bot, today)
    _, _, today_bar, _ = loaded
    if today_bar is None:
        raise ApiError(409, "market_not_open", "오늘 시가가 아직 없습니다")
    d = decide(bot, today, loaded)
    # mark the day as done BEFORE sending anything: a crash may skip today but can never double-order
    st["last_run_date"] = today
    bots.save(bot)

    client, seq = _account()
    m = bot["market"]
    currency = "KRW" if m == "KR" else "USD"
    current = float((client.request("GET", "/api/v1/prices", params={"symbols": bot["symbol"]}) or [{}])[0].get("lastPrice") or today_bar.close)
    c = _costs(Market(m))
    pos = st["position"]
    # Fills only reach `pos` at the next reconcile, so track what today's sent orders will leave us
    # holding — e.g. volatility breakout sells at the open and may re-enter on a breakout the same day.
    expect_holding = pos["qty"] > 0
    for n, o in enumerate(d["orders"]):
        if (o.side == "buy" and expect_holding) or (o.side == "sell" and not expect_holding):
            continue
        side = "BUY" if o.side == "buy" else "SELL"
        try:
            level = _level(o, today_bar.open) if o.kind == "stop" else None
            if side == "BUY":
                bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
                budget = min(max(bot["capital"] - pos["qty"] * pos["entry_price"], 0.0), bp.get("cashBuyingPower") or 0.0)
                est = level or current
                # market fills can be worse than the last price: keep >=0.5% headroom so Toss doesn't reject
                headroom = max((c.slippage_pct + c.commission_pct) / 100, LIVE_BUY_HEADROOM)
                qty = math.floor(budget / (est * (1 + headroom)))
                if o.qty is not None:
                    qty = min(qty, int(o.qty))
            else:
                sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": bot["symbol"]},
                                                 account_seq=seq) or {}).get("sellableQuantity") or 0)
                qty = int(min(pos["qty"], sellable))
            if qty <= 0:
                bots.log(bot, "skip", f"{'자금' if side == 'BUY' else '매도 가능 수량'} 부족으로 주문하지 않았습니다", "warn")
                continue
            crossed = level is not None and ((side == "BUY" and max(today_bar.open, current) >= level)
                                             or (side == "SELL" and min(today_bar.open, current) <= level))
            if o.kind == "market" or crossed:
                r = _send_market(bot, side, qty, current, _client_order_id(bot, today, n, "mkt"))
                st["pending"].append({"kind": "order", "id": r["orderId"], "side": side, "date": today, "synced": 0.0})
                bots.log(bot, "order", f"{'매수' if side == 'BUY' else '매도'} 시장가 {qty}주 주문 — {o.reason}")
                expect_holding = side == "BUY"
            else:
                r = _send_conditional(bot, side, qty, level, today, _client_order_id(bot, today, n, "cond"))
                st["pending"].append({"kind": "conditional", "id": r["conditionalOrderId"], "side": side, "date": today, "synced": 0.0})
                bots.log(bot, "order", f"{describe(o, today_bar.open)} → 조건주문 {qty}주 (오늘 하루 유효)")
        except ApiError as e:
            bots.log(bot, "error", f"주문 실패: {e.message}", "error", {"code": e.code})
    if not d["orders"]:
        bots.log(bot, "decision", f"{d['bar_date']} 종가 기준 신호 없음")
    st["last_price"] = current
    st["equity"].append({"date": today, "value": bot["capital"] + st.get("realized", 0.0) + pos["qty"] * (current - pos["entry_price"])})
    bots.save(bot)
    return {"ran": today}


def run(bot: dict, today: str) -> dict:
    return run_paper(bot, today) if bot["mode"] == "paper" else run_live(bot, today)


def cancel_live_pending(bot: dict) -> int:
    """Kill switch helper: cancel the bot's open Toss orders / conditional orders (best effort)."""
    st = bot["state"]["live"]
    client, seq = _account()
    n = 0
    for p in st["pending"]:
        try:
            if p["kind"] == "conditional":
                client.request("DELETE", f"/api/v1/conditional-orders/{p['id']}", account_seq=seq)
            else:
                client.request("POST", f"/api/v1/orders/{p['id']}/cancel", json={}, account_seq=seq)
            n += 1
        except ApiError as e:
            bots.log(bot, "error", f"취소 실패: {e.message}", "warn")
    return n

