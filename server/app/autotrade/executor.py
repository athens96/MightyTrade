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
from datetime import datetime, timedelta, timezone
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
SELL_SETTLE_POLLS = 5
SELL_SETTLE_INTERVAL = 0.5
EOD_FLATTEN = timedelta(minutes=10)  # flatten intraday daytrade positions before regular close


def is_intraday(bot: dict) -> bool:
    cls = STRATEGIES.get(bot.get("strategy", ""))
    return cls is not None and getattr(cls, "timeframe", "1d") != "1d"


def day_of(t: int) -> str:
    return datetime.fromtimestamp(t, timezone.utc).date().isoformat()


# ------------------------------------------------------------------ decide (pure)
def _strategy_atr(strat: Any) -> Optional[float]:
    atr = getattr(strat, "atr", None)
    if isinstance(atr, list) and atr and atr[-1] is not None:
        return float(atr[-1])
    return None


def _load_start(bot: dict, strat: Any) -> int:
    """History start timestamp: enough bars before today AND before an open position's entry_date."""
    anchor = time.time()
    pos = bots.state(bot)["position"]
    if pos.get("qty", 0) > 0 and pos.get("entry_date"):
        try:
            entry_ts = datetime.fromisoformat(pos["entry_date"]).replace(tzinfo=timezone.utc).timestamp()
            anchor = min(anchor, entry_ts)
        except ValueError:
            pass
    return int(anchor - (strat.warmup() * 1.6 + 40) * 86400)


def load(bot: dict, today: str) -> tuple:
    cls = STRATEGIES[bot["strategy"]]
    strat = cls(bot["params"])
    market = Market(bot["market"])
    start = _load_start(bot, strat)
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
        entry_date = pos.get("entry_date")
        if entry_date is None:
            entry_index = len(days) - 1
        elif entry_date < days[0]:
            entry_index = -1  # entry predates loaded history: avoid wrapping to negative index
        else:
            entry_index = bisect.bisect_left(days, entry_date)
    position = Position(pos["qty"], pos["entry_price"], entry_index, entry_atr=st.get("entry_atr"))
    last = today_bar.close if today_bar else bars.close[-1]
    cash = st["cash"] if bot["mode"] == "paper" else max(bot["capital"] - pos["qty"] * pos["entry_price"], 0.0)
    strat.prepare(bars)
    planned = strat.on_bar(len(bars) - 1, Context(position, cash, cash + pos["qty"] * last)) or []
    return {"date": today, "bar_date": day_of(bars.time[-1]), "orders": planned, "today_bar": today_bar,
            "candles": candles, "position": pos, "cash": cash, "last": last, "entry_atr": _strategy_atr(strat)}


def describe(o: Order, today_open: Optional[float]) -> str:
    side = "매수" if o.side == "buy" else "매도"
    if o.kind == "market":
        return f"{side} (시장가) — {o.reason}"
    if o.kind == "limit" and o.limit is not None:
        return f"{side} 지정가: {'≤' if o.side == 'buy' else '≥'} {o.limit:,.2f} — {o.reason}"
    if o.stop is not None:
        return f"{side} 조건: {'≥' if o.side == 'buy' else '≤'} {o.stop:,.2f} — {o.reason}"
    level = f"{today_open + (o.stop_from_open or 0):,.2f}" if today_open else f"시가 + {o.stop_from_open:,.2f}"
    return f"{side} 조건: ≥ {level} (시가 + {o.stop_from_open:,.2f}) — {o.reason}"


def load_intraday(bot: dict) -> tuple:
    cls = STRATEGIES[bot["strategy"]]
    strat = cls(bot["params"])
    market = Market(bot["market"])
    interval = Interval(cls.timeframe)
    candles = get_provider(market).get_candles(market, bot["symbol"], interval, limit=200)
    if len(candles) < strat.warmup() + 2:
        raise ApiError(400, "not_enough_data", "분봉 전략 계산에 필요한 과거 데이터가 부족합니다")
    return candles, Bars.from_candles(candles[:-1]), candles[-1], strat


def decide_intraday(bot: dict, today: str, loaded: Optional[tuple] = None) -> dict:
    candles, bars, latest_bar, strat = loaded or load_intraday(bot)
    st = bots.state(bot)
    pos = st["position"]
    entry_index = -1
    if pos["qty"] > 0:
        entry_t = st.get("entry_bar_time")
        if entry_t is not None and entry_t >= bars.time[0]:
            entry_index = bisect.bisect_left(bars.time, entry_t)
    position = Position(pos["qty"], pos["entry_price"], entry_index, entry_atr=st.get("entry_atr"))
    last = latest_bar.close
    cash = st["cash"] if bot["mode"] == "paper" else max(bot["capital"] - pos["qty"] * pos["entry_price"], 0.0)
    strat.prepare(bars)
    planned = strat.on_bar(len(bars) - 1, Context(position, cash, cash + pos["qty"] * last)) or []
    return {"date": today, "bar_date": datetime.fromtimestamp(bars.time[-1], timezone.utc).strftime("%Y-%m-%d %H:%M"),
            "bar_time": bars.time[-1], "orders": planned, "today_bar": latest_bar, "candles": candles,
            "position": pos, "cash": cash, "last": last, "entry_atr": _strategy_atr(strat)}


def preview(bot: dict, today: str) -> dict:
    d = decide_intraday(bot, today) if is_intraday(bot) else decide(bot, today)
    tb = d["today_bar"]
    return {
        "date": today, "bar_date": d["bar_date"], "position": d["position"], "cash": d["cash"],
        "today_open": tb.open if tb else None, "last": d["last"],
        "orders": [{"side": o.side, "kind": o.kind, "stop": o.stop, "stop_from_open": o.stop_from_open,
                    "limit": o.limit, "qty": o.qty, "reason": o.reason, "text": describe(o, tb.open if tb else None)}
                   for o in d["orders"]],
    }


def _level(o: Order, today_open: float) -> float:
    return o.stop if o.stop is not None else today_open + (o.stop_from_open or 0.0)


def _costs(market: Market) -> Costs:
    d = cost_defaults(market)
    return Costs(commission_pct=d["commission_pct"], tax_pct=d["tax_pct"], slippage_pct=d["slippage_pct"])


# ------------------------------------------------------------------ paper
def _paper_buy(st: dict, qty: int, price: float, c: Costs, date: str, reason: str,
               entry_atr: Optional[float] = None) -> None:
    px = price * (1 + c.slippage_pct / 100)
    value = qty * px
    fee = value * c.commission_pct / 100
    st["cash"] -= value + fee
    st["position"] = {"qty": qty, "entry_price": px, "entry_date": date}
    if entry_atr is not None:
        st["entry_atr"] = entry_atr
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
    st.pop("entry_atr", None)


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
        kind = p.get("kind", "stop")
        if kind == "limit":
            if p["side"] == "buy" and not holding and bar.low <= level:
                fill = min(bar.open, level)
                qty = _paper_qty(st, Order("buy", qty=p.get("qty")), fill, c)
                if qty > 0:
                    _paper_buy(st, qty, fill, c, p["active_date"], p["reason"], entry_atr=p.get("entry_atr"))
                    bots.log(bot, "fill", f"[모의] 지정가 매수 체결 {qty}주 @ {fill:,.2f} ({p['active_date']})")
            elif p["side"] == "sell" and holding and bar.high >= level:
                fill = max(bar.open, level)
                _paper_sell(st, fill, c, p["active_date"], p["reason"])
                bots.log(bot, "fill", f"[모의] 지정가 매도 체결 @ {fill:,.2f} ({p['active_date']})")
        elif p["side"] == "buy" and not holding and bar.high >= level:
            fill = max(bar.open, level)
            qty = _paper_qty(st, Order("buy", qty=p.get("qty")), fill, c)
            if qty > 0:
                _paper_buy(st, qty, fill, c, p["active_date"], p["reason"], entry_atr=p.get("entry_atr"))
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
                _paper_buy(st, qty, o_open, c, today, o.reason, entry_atr=d["entry_atr"])
                bots.log(bot, "fill", f"[모의] 매수 체결 {qty}주 @ 시가 {o_open:,.2f} — {o.reason}")
            else:
                _paper_sell(st, o_open, c, today, o.reason)
                bots.log(bot, "fill", f"[모의] 매도 체결 @ 시가 {o_open:,.2f} — {o.reason}")
        elif o.kind == "limit" and o.limit is not None:
            level = o.limit
            gapped = (o.side == "buy" and o_open <= level) or (o.side == "sell" and o_open >= level)
            if gapped:
                if o.side == "buy":
                    qty = _paper_qty(st, o, o_open, c)
                    if qty > 0:
                        _paper_buy(st, qty, o_open, c, today, o.reason, entry_atr=d["entry_atr"])
                        bots.log(bot, "fill", f"[모의] 시가가 지정가 이하라 시가 {o_open:,.2f}에 매수 {qty}주 — {o.reason}")
                else:
                    _paper_sell(st, o_open, c, today, o.reason)
                    bots.log(bot, "fill", f"[모의] 시가가 지정가 이상이라 시가 {o_open:,.2f}에 매도 — {o.reason}")
            else:
                st["pending"].append({
                    "active_date": today, "kind": "limit", "side": o.side, "level": level, "qty": o.qty, "reason": o.reason,
                    **({"entry_atr": d["entry_atr"]} if o.side == "buy" and d["entry_atr"] is not None else {}),
                })
                bots.log(bot, "order", f"[모의] {describe(o, o_open)} (오늘 하루 유효)")
        else:
            level = _level(o, o_open)
            gapped = (o.side == "buy" and o_open >= level) or (o.side == "sell" and o_open <= level)
            if gapped:
                if o.side == "buy":
                    qty = _paper_qty(st, o, o_open, c)
                    if qty > 0:
                        _paper_buy(st, qty, o_open, c, today, o.reason, entry_atr=d["entry_atr"])
                        bots.log(bot, "fill", f"[모의] 시가가 기준가를 넘어 시가 {o_open:,.2f}에 매수 {qty}주 — {o.reason}")
                else:
                    _paper_sell(st, o_open, c, today, o.reason)
                    bots.log(bot, "fill", f"[모의] 시가가 기준가 아래라 시가 {o_open:,.2f}에 매도 — {o.reason}")
            else:
                st["pending"].append({
                    "active_date": today, "side": o.side, "level": level, "qty": o.qty, "reason": o.reason,
                    **({"entry_atr": d["entry_atr"]} if o.side == "buy" and d["entry_atr"] is not None else {}),
                })
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


def _apply_live_fill(st: dict, side: str, qty: float, avg: float, date: str,
                     entry_atr: Optional[float] = None) -> None:
    pos = st["position"]
    if side == "BUY":
        new_qty = pos["qty"] + qty
        pos["entry_price"] = (pos["qty"] * pos["entry_price"] + qty * avg) / new_qty
        if pos["qty"] == 0:
            pos["entry_date"] = date
            if entry_atr is not None:
                st["entry_atr"] = entry_atr
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
            st.pop("entry_atr", None)


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
                _apply_live_fill(st, p["side"], delta, ex["averageFilledPrice"], today, entry_atr=p.get("entry_atr"))
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


def _send_limit(bot: dict, side: str, qty: int, limit_price: float, cid: str) -> dict:
    m = bot["market"]
    px = round_price(m, limit_price, "down" if side == "BUY" else "up")
    payload = {"clientOrderId": cid, "symbol": bot["symbol"], "side": side, "orderType": "LIMIT",
               "timeInForce": "DAY", "quantity": str(qty), "price": fmt_price(m, px)}
    built = {"market": m, "payload": payload, "checks": {"estimated_value": qty * px}}
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


def _wait_sell_fill(bot: dict, today: str, order_id: str) -> None:
    """Wait briefly for an open-market sell to fill before sizing a same-day follow-up buy."""
    st = bot["state"]["live"]
    for i in range(SELL_SETTLE_POLLS):
        if i and SELL_SETTLE_INTERVAL > 0:
            time.sleep(SELL_SETTLE_INTERVAL)
        reconcile_live(bot, today)
        if not any(p["kind"] == "order" and p["id"] == order_id for p in st["pending"]):
            return


def _unsynced_sell_qty(st: dict) -> float:
    return sum(
        max(float(p.get("qty") or 0.0) - float(p.get("synced") or 0.0), 0.0)
        for p in st["pending"]
        if p.get("kind") == "order" and p.get("side") == "SELL"
    )


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
    # Fills only reach `pos` at reconcile, so track what today's sent orders will leave us
    # holding — e.g. volatility breakout sells at the open and may re-enter on a breakout the same day.
    expect_holding = pos["qty"] > 0
    orders = d["orders"]
    for n, o in enumerate(orders):
        if (o.side == "buy" and expect_holding) or (o.side == "sell" and not expect_holding):
            continue
        side = "BUY" if o.side == "buy" else "SELL"
        try:
            level = o.limit if o.kind == "limit" else (_level(o, today_bar.open) if o.kind == "stop" else None)
            crossed = o.kind == "stop" and level is not None and (
                (side == "BUY" and max(today_bar.open, current) >= level)
                or (side == "SELL" and min(today_bar.open, current) <= level)
            )
            immediate = o.kind == "market" or crossed
            if side == "BUY":
                bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
                # Account for an in-flight same-day sell that has not settled into `pos` yet.
                unsynced_sell = _unsynced_sell_qty(st)
                eff_qty = max(pos["qty"] - unsynced_sell, 0.0)
                bot_budget = max(bot["capital"] - eff_qty * pos["entry_price"], 0.0)
                cash_bp = bp.get("cashBuyingPower") or 0.0
                if not immediate and unsynced_sell > 0:
                    # Conditional / limit buys fill later intraday, after the open market sell settles.
                    cash_bp += unsynced_sell * current * (1 - (c.commission_pct + c.tax_pct) / 100)
                budget = min(bot_budget, cash_bp)
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
            extra = {"entry_atr": d["entry_atr"]} if side == "BUY" and d["entry_atr"] is not None else {}
            if immediate:
                r = _send_market(bot, side, qty, current, _client_order_id(bot, today, n, "mkt"))
                st["pending"].append({"kind": "order", "id": r["orderId"], "side": side, "qty": qty,
                                      "date": today, "synced": 0.0, **extra})
                bots.log(bot, "order", f"{'매수' if side == 'BUY' else '매도'} 시장가 {qty}주 주문 — {o.reason}")
                if side == "SELL" and any(nxt.side == "buy" for nxt in orders[n + 1:]):
                    _wait_sell_fill(bot, today, r["orderId"])
                    sell_in_flight = any(p["kind"] == "order" and p["id"] == r["orderId"] for p in st["pending"])
                    expect_holding = pos["qty"] > 0 and not sell_in_flight
                else:
                    expect_holding = side == "BUY"
            elif o.kind == "limit" and level is not None:
                r = _send_limit(bot, side, qty, level, _client_order_id(bot, today, n, "lmt"))
                st["pending"].append({"kind": "order", "id": r["orderId"], "side": side, "qty": qty,
                                      "date": today, "synced": 0.0, **extra})
                bots.log(bot, "order", f"{describe(o, today_bar.open)} → 지정가 {qty}주 (오늘 하루 유효)")
            else:
                r = _send_conditional(bot, side, qty, level, today, _client_order_id(bot, today, n, "cond"))
                st["pending"].append({"kind": "conditional", "id": r["conditionalOrderId"], "side": side, "qty": qty,
                                      "date": today, "synced": 0.0, **extra})
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


# ------------------------------------------------------------------ intraday daytrade
def _record_day_equity(st: dict, today: str, value: float) -> None:
    if st["equity"] and st["equity"][-1]["date"] == today:
        st["equity"][-1]["value"] = value
    else:
        st["equity"].append({"date": today, "value": value})


def run_intraday(bot: dict, today: str, now: datetime, session_end: datetime) -> dict:
    """Bar-by-bar intraday execution (5m/15m daytrade bots) with mandatory pre-close flattening."""
    st = bots.state(bot)
    if st.get("intraday_closed_date") == today:
        return {"skipped": "closed_for_day"}
    market = Market(bot["market"])
    c = _costs(market)

    # Mandatory end-of-day flatten: cancel open entries and liquidate any open intraday position.
    if now >= session_end - EOD_FLATTEN:
        if bot["mode"] == "paper":
            st["pending"] = []
            if st["position"]["qty"] > 0:
                candles = get_provider(market).get_candles(market, bot["symbol"], Interval(STRATEGIES[bot["strategy"]].timeframe), limit=5)
                px = candles[-1].close if candles else (st.get("last_price") or st["position"]["entry_price"])
                _paper_sell(st, px, c, today, "장 마감 전 당일 강제 청산")
                st.pop("entry_bar_time", None)
                st["last_price"] = px
                bots.log(bot, "fill", f"[모의] 장 마감 전 당일 강제 청산 @ {px:,.2f}")
            _record_day_equity(st, today, st["cash"] + st["position"]["qty"] * (st.get("last_price") or 0.0))
        else:
            reconcile_live(bot, today)
            if st["pending"]:
                cancel_live_pending(bot)
                st["pending"] = []
            pos = st["position"]
            if pos["qty"] > 0:
                client, seq = _account()
                sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": bot["symbol"]},
                                                 account_seq=seq) or {}).get("sellableQuantity") or 0)
                qty = int(min(pos["qty"], sellable))
                if qty > 0:
                    cur = float((client.request("GET", "/api/v1/prices", params={"symbols": bot["symbol"]}) or [{}])[0].get("lastPrice") or pos["entry_price"])
                    r = _send_market(bot, "SELL", qty, cur, _client_order_id(bot, today, 999, "eod"))
                    st["pending"].append({"kind": "order", "id": r["orderId"], "side": "SELL", "qty": qty, "date": today, "synced": 0.0})
                    bots.log(bot, "order", f"장 마감 전 당일 강제 청산 시장가 {qty}주")
        st["intraday_closed_date"] = today
        st["last_run_date"] = today
        bots.save(bot)
        return {"flattened": today}

    loaded = load_intraday(bot)
    candles, bars, latest_bar, _ = loaded
    bar_time = bars.time[-1]
    if st.get("last_bar_time") == bar_time:
        return {"skipped": "already_ran_bar"}

    if bot["mode"] == "paper":
        # Settle prior-bar pending stop/limit orders against the just-completed bar.
        last_done = candles[-2]
        for p in list(st["pending"]):
            lvl, holding = p["level"], st["position"]["qty"] > 0
            if p.get("kind", "stop") == "limit":
                if p["side"] == "buy" and not holding and last_done.low <= lvl:
                    qty = _paper_qty(st, Order("buy", qty=p.get("qty")), min(last_done.open, lvl), c)
                    if qty > 0:
                        _paper_buy(st, qty, min(last_done.open, lvl), c, today, p["reason"])
                        st["entry_bar_time"] = bar_time
                        bots.log(bot, "fill", f"[모의/분봉] 지정가 매수 {qty}주 @ {min(last_done.open, lvl):,.2f}")
                elif p["side"] == "sell" and holding and last_done.high >= lvl:
                    _paper_sell(st, max(last_done.open, lvl), c, today, p["reason"])
                    st.pop("entry_bar_time", None)
                    bots.log(bot, "fill", f"[모의/분봉] 지정가 매도 @ {max(last_done.open, lvl):,.2f}")
            elif p["side"] == "buy" and not holding and last_done.high >= lvl:
                qty = _paper_qty(st, Order("buy", qty=p.get("qty")), max(last_done.open, lvl), c)
                if qty > 0:
                    _paper_buy(st, qty, max(last_done.open, lvl), c, today, p["reason"])
                    st["entry_bar_time"] = bar_time
                    bots.log(bot, "fill", f"[모의/분봉] 돌파 매수 {qty}주 @ {max(last_done.open, lvl):,.2f}")
            elif p["side"] == "sell" and holding and last_done.low <= lvl:
                _paper_sell(st, min(last_done.open, lvl), c, today, p["reason"])
                st.pop("entry_bar_time", None)
                bots.log(bot, "fill", f"[모의/분봉] 스톱 매도 @ {min(last_done.open, lvl):,.2f}")
        st["pending"] = []
        d = decide_intraday(bot, today, loaded)
        px = latest_bar.open
        for o in d["orders"]:
            holding = st["position"]["qty"] > 0
            if (o.side == "buy" and holding) or (o.side == "sell" and not holding):
                continue
            if o.kind == "market":
                if o.side == "buy":
                    qty = _paper_qty(st, o, px, c)
                    if qty > 0:
                        _paper_buy(st, qty, px, c, today, o.reason)
                        st["entry_bar_time"] = bar_time
                        bots.log(bot, "fill", f"[모의/분봉] 매수 {qty}주 @ {px:,.2f} — {o.reason}")
                else:
                    _paper_sell(st, px, c, today, o.reason)
                    st.pop("entry_bar_time", None)
                    bots.log(bot, "fill", f"[모의/분봉] 매도 @ {px:,.2f} — {o.reason}")
            else:
                lvl = o.limit if o.kind == "limit" else _level(o, px)
                st["pending"].append({"active_date": today, "kind": o.kind, "side": o.side, "level": lvl, "qty": o.qty, "reason": o.reason})
        st["last_bar_time"] = bar_time
        st["last_run_date"] = today
        st["last_price"] = latest_bar.close
        _record_day_equity(st, today, st["cash"] + st["position"]["qty"] * latest_bar.close)
        bots.save(bot)
        return {"ran_bar": bar_time}

    # Live intraday execution: mark bar_time before sending to prevent duplicate bar orders.
    reconcile_live(bot, today)
    d = decide_intraday(bot, today, loaded)
    st["last_bar_time"] = bar_time
    st["last_run_date"] = today
    bots.save(bot)

    client, seq = _account()
    m = bot["market"]
    currency = "KRW" if m == "KR" else "USD"
    current = float((client.request("GET", "/api/v1/prices", params={"symbols": bot["symbol"]}) or [{}])[0].get("lastPrice") or latest_bar.close)
    pos = st["position"]
    # Replace stale intraday conditional orders before placing fresh bar orders.
    if d["orders"] and any(p["kind"] == "conditional" for p in st["pending"]):
        keep = []
        for p in st["pending"]:
            if p["kind"] == "conditional":
                try:
                    client.request("DELETE", f"/api/v1/conditional-orders/{p['id']}", account_seq=seq)
                except ApiError:
                    keep.append(p)
            else:
                keep.append(p)
        st["pending"] = keep

    for n, o in enumerate(d["orders"]):
        holding = pos["qty"] > 0
        if (o.side == "buy" and holding) or (o.side == "sell" and not holding):
            continue
        side = "BUY" if o.side == "buy" else "SELL"
        try:
            level = o.limit if o.kind == "limit" else (_level(o, latest_bar.open) if o.kind == "stop" else None)
            crossed = o.kind == "stop" and level is not None and (
                (side == "BUY" and current >= level) or (side == "SELL" and current <= level)
            )
            if side == "BUY":
                bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
                budget = min(max(bot["capital"] - pos["qty"] * pos["entry_price"], 0.0), bp.get("cashBuyingPower") or 0.0)
                headroom = max((c.slippage_pct + c.commission_pct) / 100, LIVE_BUY_HEADROOM)
                qty = math.floor(budget / ((level or current) * (1 + headroom)))
                if o.qty is not None:
                    qty = min(qty, int(o.qty))
            else:
                sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": bot["symbol"]},
                                                 account_seq=seq) or {}).get("sellableQuantity") or 0)
                qty = int(min(pos["qty"], sellable))
            if qty <= 0:
                continue
            tag_id = f"{today}:{bar_time}"
            if o.kind == "market" or crossed:
                r = _send_market(bot, side, qty, current, _client_order_id(bot, tag_id, n, "imkt"))
                st["pending"].append({"kind": "order", "id": r["orderId"], "side": side, "qty": qty, "date": today, "synced": 0.0})
                bots.log(bot, "order", f"[분봉] {'매수' if side == 'BUY' else '매도'} 시장가 {qty}주 — {o.reason}")
            elif o.kind == "limit" and level is not None:
                r = _send_limit(bot, side, qty, level, _client_order_id(bot, tag_id, n, "ilmt"))
                st["pending"].append({"kind": "order", "id": r["orderId"], "side": side, "qty": qty, "date": today, "synced": 0.0})
                bots.log(bot, "order", f"[분봉] {describe(o, latest_bar.open)} → 지정가 {qty}주")
            else:
                r = _send_conditional(bot, side, qty, level, today, _client_order_id(bot, tag_id, n, "icond"))
                st["pending"].append({"kind": "conditional", "id": r["conditionalOrderId"], "side": side, "qty": qty, "date": today, "synced": 0.0})
                bots.log(bot, "order", f"[분봉] {describe(o, latest_bar.open)} → 조건주문 {qty}주")
        except ApiError as e:
            bots.log(bot, "error", f"[분봉] 주문 실패: {e.message}", "error", {"code": e.code})
    st["last_price"] = current
    _record_day_equity(st, today, bot["capital"] + st.get("realized", 0.0) + pos["qty"] * (current - pos["entry_price"]))
    bots.save(bot)
    return {"ran_bar": bar_time}


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

