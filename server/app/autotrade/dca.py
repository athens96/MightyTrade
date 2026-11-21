"""Smart Dollar-Cost Averaging ('스마트 적립식 모으기') bot executor (MT-7).

Accumulates shares on a recurring schedule (daily / weekly / biweekly / monthly) within a total
`capital` ceiling, automatically scaling each purchase up in oversold dips (`dip_mult`, e.g. 1.5x)
and down in overheated rallies (`peak_mult`, e.g. 0.5x), while tracking average-cost savings against
plain fixed-amount DCA.
"""
from __future__ import annotations

import hashlib
import math
import time
from datetime import date, datetime, timezone
from typing import Any, Optional

from ..account.toss_account import _account, normalize
from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from ..quant import indicators as ind
from ..quant import market_timing
from ..quant.data import cost_defaults
from ..quant.engine import Bars, Costs
from ..trading import orders as trading
from . import bots, protections

LIVE_BUY_HEADROOM = 0.005

DCA_CADENCES = {
    "daily": "매일",
    "weekly": "매주",
    "biweekly": "격주 (2주)",
    "monthly": "매월 초",
}


def _costs(market: Market) -> Costs:
    d = cost_defaults(market)
    return Costs(commission_pct=d["commission_pct"], tax_pct=d["tax_pct"], slippage_pct=d["slippage_pct"])


def _client_order_id(bot: dict, today: str) -> str:
    return hashlib.sha256(f"{bot['id']}:{today}:dca:BUY".encode()).hexdigest()[:32]


def _day_of(t: int) -> str:
    return datetime.fromtimestamp(t, timezone.utc).date().isoformat()


def should_dca(today: str, last_dca_date: Optional[str], cadence: str, weekday: int = 0) -> Optional[str]:
    try:
        dt = date.fromisoformat(today)
    except ValueError:
        return None
    if last_dca_date is None:
        if cadence in ("weekly", "biweekly") and dt.weekday() != weekday:
            # On initial run, wait for chosen weekday unless already past? Actually allow immediate first accumulation if weekday matches, else wait
            return None
        return f"최초 적립식 매수 ({DCA_CADENCES.get(cadence, cadence)})"
    try:
        dl = date.fromisoformat(last_dca_date)
    except ValueError:
        return None
    if dt <= dl:
        return None
    if cadence == "daily":
        return "일간 정기 적립"
    if cadence == "weekly":
        if dt.weekday() == weekday and dt.isocalendar()[:2] != dl.isocalendar()[:2]:
            return "주간 정기 적립"
        # Also catch holiday-shifted week: if weekday passed and this ISO week hasn't bought yet
        if dt.weekday() > weekday and dt.isocalendar()[:2] != dl.isocalendar()[:2]:
            return "주간 정기 적립 (휴장일 순연)"
    elif cadence == "biweekly":
        if (dt - dl).days >= 13 and dt.weekday() >= weekday:
            return "격주 정기 적립"
    elif cadence == "monthly":
        if (dt.year, dt.month) != (dl.year, dl.month):
            return "월간 정기 적립"
    return None


def compute_dca_multiplier(
    bars: Bars,
    smart_scale: bool = True,
    dip_mult: float = 1.5,
    peak_mult: float = 0.5,
) -> tuple[float, str]:
    if not smart_scale or len(bars) < 20:
        return 1.0, "정액 적립 (1.0x)"
    closes = bars.close
    last = closes[-1]
    rsi_vals = ind.rsi(closes, 14)
    ma20_vals = ind.sma(closes, 20)
    r14 = rsi_vals[-1] if rsi_vals and rsi_vals[-1] is not None else 50.0
    ma20 = ma20_vals[-1] if ma20_vals and ma20_vals[-1] is not None else last

    if r14 <= 32.0 or (ma20 > 0 and last <= ma20 * 0.94):
        return dip_mult, f"세일 구간 확대 적립 ({dip_mult:g}x · RSI {r14:.0f})"
    if r14 <= 42.0 or (ma20 > 0 and last <= ma20 * 0.97):
        mid = round(1.0 + (dip_mult - 1.0) * 0.5, 2)
        return mid, f"눌림목 가산 적립 ({mid:g}x · RSI {r14:.0f})"
    if r14 >= 70.0 or (ma20 > 0 and last >= ma20 * 1.07):
        return peak_mult, f"과열 구간 축소 적립 ({peak_mult:g}x · RSI {r14:.0f})"
    return 1.0, f"기본 정액 적립 (1.0x · RSI {r14:.0f})"


def decide(bot: dict, today: str) -> dict[str, Any]:
    st = bots.state(bot)
    market = Market(bot["market"])
    start = int(time.time() - 150 * 86400)
    candles = get_provider(market).get_history(market, bot["symbol"], Interval.D1, start)
    past = [c for c in candles if _day_of(c.time) < today]
    today_bar = next((c for c in candles if _day_of(c.time) == today), None)
    if len(past) < 5:
        raise ApiError(400, "not_enough_data", "적립식 판단에 필요한 과거 시세가 부족합니다")

    bars = Bars.from_candles(past)
    cadence = str(bot.get("rebalance") or "weekly")
    weekday = int(bot.get("weekday", 0))
    due_reason = should_dca(today, st.get("last_dca_date"), cadence, weekday)

    mult, mult_note = compute_dca_multiplier(
        bars,
        smart_scale=bool(bot.get("smart_scale", True)),
        dip_mult=float(bot.get("dip_mult", 1.5)),
        peak_mult=float(bot.get("peak_mult", 0.5)),
    )

    mt = bot.get("market_timing") or {}
    mt_snap = None
    if (mt.get("mode") or "off") != "off":
        mt_snap = market_timing.current_regime_snapshot(
            bot["market"], mt.get("benchmark"), int(mt.get("ma_short", 20)), int(mt.get("ma_long", 60)), before_date=today
        )
        mt_scale = market_timing.buy_scale_for(mt["mode"], mt_snap["regime"])
        mult = round(mult * mt_scale, 3)

    pos = st["position"]
    px_ref = today_bar.open if today_bar else bars.close[-1]
    last_px = today_bar.close if today_bar else bars.close[-1]
    c = _costs(market)
    headroom = max((c.slippage_pct + c.commission_pct) / 100.0, LIVE_BUY_HEADROOM)

    invested_so_far = float(st.get("dca_invested") or (pos["qty"] * pos["entry_price"]))
    remaining_cap = max(float(bot["capital"]) - invested_so_far, 0.0)
    cash = float(st["cash"]) if bot["mode"] == "paper" else remaining_cap
    target_spend = min(float(bot["base_amount"]) * mult, remaining_cap, cash)
    qty = math.floor(target_spend / (px_ref * (1.0 + headroom))) if px_ref > 0 and target_spend > 0 else 0

    orders = []
    if due_reason and qty > 0:
        orders.append({
            "side": "buy",
            "kind": "market",
            "symbol": bot["symbol"],
            "qty": qty,
            "price": px_ref,
            "reason": f"{due_reason} · {mult_note}",
            "text": f"{bot['symbol']} 적립 매수 {qty}주 — {mult_note}",
        })

    return {
        "date": today,
        "bar_date": _day_of(bars.time[-1]),
        "due": due_reason is not None,
        "reason": f"{due_reason} · {mult_note}" if due_reason else f"적립 대기 ({mult_note})",
        "multiplier": mult,
        "multiplier_note": mult_note,
        "position": pos,
        "cash": cash,
        "today_bar": today_bar,
        "today_open": today_bar.open if today_bar else None,
        "last": last_px,
        "market_regime": mt_snap,
        "orders": orders,
    }


def preview(bot: dict, today: str) -> dict[str, Any]:
    d = decide(bot, today)
    return {k: v for k, v in d.items() if k != "today_bar"}


def _update_dca_benchmark(st: dict, base_amount: float, ref_price: float) -> None:
    if ref_price <= 0 or base_amount <= 0:
        return
    st["dca_steps"] = int(st.get("dca_steps") or 0) + 1
    st["reg_dca_invested"] = float(st.get("reg_dca_invested") or 0.0) + base_amount
    st["reg_dca_shares"] = float(st.get("reg_dca_shares") or 0.0) + (base_amount / ref_price)


def public_dca_stats(st: dict) -> dict[str, Any]:
    pos = st["position"]
    smart_avg = float(pos["entry_price"]) if pos["qty"] > 0 else None
    reg_shares = float(st.get("reg_dca_shares") or 0.0)
    reg_inv = float(st.get("reg_dca_invested") or 0.0)
    reg_avg = (reg_inv / reg_shares) if reg_shares > 0 else None
    saved_pct = ((1.0 - smart_avg / reg_avg) * 100.0) if (smart_avg and reg_avg and reg_avg > 0) else None
    return {
        "steps": int(st.get("dca_steps") or 0),
        "total_invested": round(float(st.get("dca_invested") or 0.0), 2),
        "last_dca_date": st.get("last_dca_date"),
        "smart_avg_price": round(smart_avg, 2) if smart_avg is not None else None,
        "regular_avg_price": round(reg_avg, 2) if reg_avg is not None else None,
        "saved_pct": round(saved_pct, 2) if saved_pct is not None else None,
    }


def run_paper(bot: dict, today: str) -> dict[str, Any]:
    st = bots.state(bot)
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    d = decide(bot, today)
    tb = d["today_bar"]
    if tb is None:
        raise ApiError(409, "market_not_open", "오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다")

    c = _costs(Market(bot["market"]))
    buy_block = protections.check_buy_allowed(bot, st, today, current_equity=st["cash"] + st["position"]["qty"] * tb.open)

    if d["due"]:
        _update_dca_benchmark(st, float(bot["base_amount"]), tb.open)
        if buy_block:
            bots.log(bot, "skip", f"[모의/적립] 신규 매수 보류: {buy_block}", "warn")
        elif d["orders"]:
            o = d["orders"][0]
            px = tb.open * (1.0 + c.slippage_pct / 100.0)
            unit = px * (1.0 + c.commission_pct / 100.0)
            qty = min(int(o["qty"]), math.floor(st["cash"] / unit)) if unit > 0 else 0
            if qty > 0:
                cost = qty * unit
                st["cash"] -= cost
                pos = st["position"]
                old_q = pos["qty"]
                new_q = old_q + qty
                avg = (old_q * pos["entry_price"] + qty * px) / new_q
                st["position"] = {"qty": new_q, "entry_price": avg, "entry_date": pos["entry_date"] or today}
                st["dca_invested"] = float(st.get("dca_invested") or 0.0) + cost
                st["last_dca_date"] = today
                st["trades"].append({
                    "entry_date": today, "entry_price": px, "qty": qty, "entry_reason": o["reason"],
                    "cost_basis": cost, "exit_date": None, "exit_price": None, "exit_reason": "", "pnl": None, "return_pct": None,
                })
                bots.log(bot, "fill", f"[모의/적립] {qty}주 매수 @ {px:,.2f} (평균단가 {avg:,.2f}) — {d['multiplier_note']}")
            else:
                st["last_dca_date"] = today
                bots.log(bot, "skip", f"[모의/적립] 1주 미만 금액이거나 잔고 부족으로 이번 회차 생략 ({d['multiplier_note']})", "info")
        else:
            st["last_dca_date"] = today
            bots.log(bot, "decision", f"[모의/적립] 배수 0x 또는 한도 도달로 이번 회차 매수 생략 ({d['multiplier_note']})")

    st["last_run_date"] = today
    st["last_price"] = tb.close
    pos = st["position"]
    st["equity"].append({"date": today, "value": st["cash"] + pos["qty"] * tb.close})
    protections.evaluate_after_step(bot, st, today)
    bots.save(bot)
    return {"ran": today, "dca_due": d["due"]}


def run_live(bot: dict, today: str) -> dict[str, Any]:
    from . import executor

    st = bot["state"]["live"]
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    executor.reconcile_live(bot, today)
    d = decide(bot, today)
    if d["today_bar"] is None:
        raise ApiError(409, "market_not_open", "오늘 시가가 아직 없습니다")

    st["last_run_date"] = today
    bots.save(bot)

    client, seq = _account()
    m = bot["market"]
    currency = "KRW" if m == "KR" else "USD"
    current = float((client.request("GET", "/api/v1/prices", params={"symbols": bot["symbol"]}) or [{}])[0].get("lastPrice") or d["last"])
    pos = st["position"]
    c = _costs(Market(m))
    headroom = max((c.slippage_pct + c.commission_pct) / 100.0, LIVE_BUY_HEADROOM)
    buy_block = protections.check_buy_allowed(bot, st, today)

    if d["due"]:
        _update_dca_benchmark(st, float(bot["base_amount"]), current)
        if buy_block:
            bots.log(bot, "skip", f"[적립] 신규 매수 보류: {buy_block}", "warn")
        elif d["multiplier"] > 0:
            bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
            cash_bp = float(bp.get("cashBuyingPower") or 0.0)
            invested = pos["qty"] * pos["entry_price"]
            rem_cap = max(float(bot["capital"]) - invested, 0.0)
            spend = min(float(bot["base_amount"]) * float(d["multiplier"]), rem_cap, cash_bp)
            qty = math.floor(spend / (current * (1.0 + headroom))) if current > 0 and spend > 0 else 0
            if qty > 0:
                try:
                    cid = _client_order_id(bot, today)
                    payload = {"clientOrderId": cid, "symbol": bot["symbol"], "side": "BUY", "orderType": "MARKET",
                               "timeInForce": "DAY", "quantity": str(qty)}
                    built = {"market": m, "payload": payload, "checks": {"estimated_value": qty * current}}
                    r = trading.submit(built, bot["user_id"], source=f"bot:{bot['id']}")
                    st["pending"].append({"kind": "order", "id": r["orderId"], "side": "BUY", "qty": qty, "date": today, "synced": 0.0})
                    st["dca_invested"] = float(st.get("dca_invested") or 0.0) + qty * current
                    st["last_dca_date"] = today
                    bots.log(bot, "order", f"[적립] 시장가 매수 {qty}주 — {d['multiplier_note']}")
                except ApiError as e:
                    bots.log(bot, "error", f"[적립] 주문 실패: {e.message}", "error", {"code": e.code})
            else:
                st["last_dca_date"] = today
                bots.log(bot, "skip", "[적립] 매수 가능 현금 또는 잔여 한도 부족으로 생략", "warn")

    st["last_price"] = current
    st["equity"].append({"date": today, "value": float(bot["capital"]) + st.get("realized", 0.0) + pos["qty"] * (current - pos["entry_price"])})
    protections.evaluate_after_step(bot, st, today)
    bots.save(bot)
    return {"ran": today, "dca_due": d["due"]}


def run(bot: dict, today: str) -> dict[str, Any]:
    return run_paper(bot, today) if bot["mode"] == "paper" else run_live(bot, today)
