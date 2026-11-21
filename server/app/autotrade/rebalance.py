"""Multi-asset portfolio rebalancing bot executor (paper & live).

Uses the exact same portfolio allocation models and rebalance rules as Quant Lab
(`app/quant/portfolio.py`). Live execution sends SELL orders first, waits briefly for fills,
then sends BUY orders within capital/buying-power/safety limits.
"""
from __future__ import annotations

import hashlib
import math
import time
from typing import Any, Optional

from ..account.toss_account import _account, normalize
from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from ..quant import portfolio
from ..quant.data import cost_defaults
from ..quant.engine import Bars, Costs
from ..trading import orders as trading
from ..users.repository import now_iso
from . import bots, protections

TERMINAL = {"FILLED", "CANCELED", "REJECTED", "REPLACED"}
LIVE_BUY_HEADROOM = 0.005
SELL_SETTLE_POLLS = 5
SELL_SETTLE_INTERVAL = 0.5


def _costs(market: Market) -> Costs:
    d = cost_defaults(market)
    return Costs(commission_pct=d["commission_pct"], tax_pct=d["tax_pct"], slippage_pct=d["slippage_pct"])


def _client_order_id(bot: dict, today: str, symbol: str, side: str) -> str:
    return hashlib.sha256(f"{bot['id']}:{today}:rebal:{symbol}:{side}".encode()).hexdigest()[:32]


def _day_of(t: int) -> str:
    return portfolio._day_str(t)


# ------------------------------------------------------------------ load & decide (pure)
def load(bot: dict, today: str) -> tuple:
    model = portfolio.PORTFOLIO_MODELS[bot["model"]]
    params = model.resolve(bot.get("params"))
    market = Market(bot["market"])
    assets = bot["assets"]
    warm = model.warmup(params)
    start = int(time.time() - (warm * 1.6 + 45) * 86400)
    provider = get_provider(market)

    past_bars: dict[str, Bars] = {}
    today_bars: dict[str, Any] = {}
    for a in assets:
        sym = a["symbol"]
        candles = provider.get_history(market, sym, Interval.D1, start)
        past = [c for c in candles if _day_of(c.time) < today]
        tb = next((c for c in candles if _day_of(c.time) == today), None)
        if len(past) < warm + 1:
            raise ApiError(400, "not_enough_data", f"{sym}: 포트폴리오 계산에 필요한 과거 데이터가 부족합니다")
        past_bars[sym] = Bars.from_candles(past)
        if tb is not None:
            today_bars[sym] = tb

    times, _, closes = portfolio.align_bars(past_bars)
    if len(times) < warm + 1:
        raise ApiError(400, "not_enough_data", "자산 간 공통 거래일이 부족합니다")
    return times, closes, today_bars, model, params


def decide(bot: dict, today: str, loaded: Optional[tuple] = None) -> dict[str, Any]:
    times, closes, today_bars, model, params = loaded or load(bot, today)
    st = bots.state(bot)
    assets = bot["assets"]
    syms = [a["symbol"] for a in assets]
    positions = st.get("positions") or {}

    last_prices = {s: (today_bars[s].close if s in today_bars else closes[s][-1]) for s in syms}
    open_prices = {s: (today_bars[s].open if s in today_bars else last_prices[s]) for s in syms}
    pos_val = sum((positions.get(s, {}).get("qty") or 0) * last_prices[s] for s in syms)
    if bot["mode"] == "paper":
        total_val = float(st.get("cash", bot["capital"])) + pos_val
    else:
        total_val = max(float(bot["capital"]), pos_val)

    cur_w = {s: ((positions.get(s, {}).get("qty") or 0) * last_prices[s] / total_val if total_val > 0 else 0.0) for s in syms}
    tgt_w = model.target_weights(closes, len(times) - 1, assets, params)
    reason = portfolio.should_rebalance(
        today, st.get("last_rebalance_date"), bot["rebalance"], cur_w, tgt_w, float(bot.get("drift_pct", 5.0))
    )

    c = _costs(Market(bot["market"]))
    headroom = max((c.slippage_pct + c.commission_pct) / 100.0, LIVE_BUY_HEADROOM)
    sells, buys = [], []
    for a in assets:
        s = a["symbol"]
        cur_qty = int(positions.get(s, {}).get("qty") or 0)
        w = tgt_w.get(s, 0.0)
        px = open_prices[s]
        tgt_qty = math.floor(total_val * w / (px * (1.0 + headroom))) if w > 0 and px > 0 else 0
        diff = tgt_qty - cur_qty
        if diff < 0:
            sells.append({
                "symbol": s, "side": "sell", "kind": "market", "qty": -diff,
                "price": px, "current_pct": round(cur_w[s] * 100, 1), "target_pct": round(w * 100, 1),
                "text": f"{s} 매도 {-diff}주 (현재 {cur_w[s] * 100:.1f}% → 목표 {w * 100:.1f}%)",
            })
        elif diff > 0:
            buys.append({
                "symbol": s, "side": "buy", "kind": "market", "qty": diff,
                "price": px, "current_pct": round(cur_w[s] * 100, 1), "target_pct": round(w * 100, 1),
                "text": f"{s} 매수 {diff}주 (현재 {cur_w[s] * 100:.1f}% → 목표 {w * 100:.1f}%)",
            })

    return {
        "date": today,
        "bar_date": _day_of(times[-1]),
        "due": reason is not None,
        "reason": reason,
        "total_value": total_val,
        "last_prices": last_prices,
        "open_prices": open_prices,
        "current_weights": {s: round(cur_w[s] * 100.0, 2) for s in syms},
        "target_weights": {s: round(tgt_w.get(s, 0.0) * 100.0, 2) for s in syms},
        "target_weights_raw": tgt_w,
        "orders": sells + buys,
        "has_today_bars": len(today_bars) == len(syms),
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
        "orders": d["orders"] if d["due"] else [],
        "planned_orders": d["orders"],
    }


# ------------------------------------------------------------------ paper
def run_paper(bot: dict, today: str) -> dict[str, Any]:
    st = bots.state(bot)
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    loaded = load(bot, today)
    _, _, today_bars, _, _ = loaded
    if len(today_bars) < len(bot["assets"]):
        raise ApiError(409, "market_not_open", "일부 종목의 오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다")

    d = decide(bot, today, loaded)
    c = _costs(Market(bot["market"]))
    comm = c.commission_pct / 100.0
    tax = c.tax_pct / 100.0
    slip = c.slippage_pct / 100.0
    positions = st.setdefault("positions", {})
    syms = [a["symbol"] for a in bot["assets"]]

    if d["due"]:
        val_open = float(st["cash"]) + sum(int(positions.get(s, {}).get("qty") or 0) * d["open_prices"][s] for s in syms)
        tgt_w = d["target_weights_raw"]
        desired = {}
        for s in syms:
            px_buy = d["open_prices"][s] * (1.0 + slip) * (1.0 + comm)
            desired[s] = math.floor(val_open * tgt_w.get(s, 0.0) / px_buy) if tgt_w.get(s, 0.0) > 0 and px_buy > 0 else 0

        n_orders = 0
        # 1) Sells first
        for s in syms:
            pos = positions.get(s) or {"qty": 0, "entry_price": 0.0}
            diff = desired[s] - int(pos["qty"])
            if diff < 0:
                qty = -diff
                px = d["open_prices"][s] * (1.0 - slip)
                net = qty * px * (1.0 - comm - tax)
                st["cash"] += net
                pos["qty"] = int(pos["qty"]) - qty
                if pos["qty"] <= 0:
                    positions.pop(s, None)
                else:
                    positions[s] = pos
                n_orders += 1
                bots.log(bot, "fill", f"[모의/리밸런싱] {s} 매도 {qty}주 @ {px:,.2f}")
        # 2) Buys second
        for s in syms:
            pos = positions.get(s) or {"qty": 0, "entry_price": 0.0}
            diff = desired[s] - int(pos["qty"])
            if diff > 0:
                px = d["open_prices"][s] * (1.0 + slip)
                unit_cost = px * (1.0 + comm)
                qty = min(diff, math.floor(st["cash"] / unit_cost)) if unit_cost > 0 else 0
                if qty > 0:
                    st["cash"] -= qty * unit_cost
                    old_q = int(pos["qty"])
                    new_q = old_q + qty
                    avg = (old_q * float(pos["entry_price"]) + qty * px) / new_q
                    positions[s] = {"qty": new_q, "entry_price": avg}
                    n_orders += 1
                    bots.log(bot, "fill", f"[모의/리밸런싱] {s} 매수 {qty}주 @ {px:,.2f}")
        st["last_rebalance_date"] = today
        st["rebalances"].append({
            "date": today, "reason": d["reason"], "trades": n_orders, "weights": d["target_weights"],
        })
        bots.log(bot, "order", f"[모의] {d['reason']} 완료 ({n_orders}건)")
    else:
        bots.log(bot, "decision", f"[모의] {d['bar_date']} 기준 리밸런싱 조건 미해당 (유지)")

    st["last_run_date"] = today
    st["last_prices"] = d["last_prices"]
    st["target_weights"] = d["target_weights"]
    eq_now = float(st["cash"]) + sum(int(positions.get(s, {}).get("qty") or 0) * d["last_prices"][s] for s in syms)
    st["equity"].append({"date": today, "value": eq_now})
    protections.evaluate_after_step(bot, st, today)
    bots.save(bot)
    return {"ran": today, "rebalanced": d["due"]}


# ------------------------------------------------------------------ live
def _apply_fill(st: dict, symbol: str, side: str, qty: float, avg: float) -> None:
    positions = st.setdefault("positions", {})
    pos = positions.get(symbol) or {"qty": 0, "entry_price": 0.0}
    if side == "BUY":
        new_q = pos["qty"] + qty
        pos["entry_price"] = (pos["qty"] * pos["entry_price"] + qty * avg) / new_q
        pos["qty"] = new_q
        positions[symbol] = pos
    else:
        sold = min(qty, pos["qty"])
        st["realized"] = st.get("realized", 0.0) + (avg - pos["entry_price"]) * sold
        pos["qty"] -= sold
        if pos["qty"] <= 0:
            positions.pop(symbol, None)
        else:
            positions[symbol] = pos


def reconcile_live(bot: dict, today: str) -> bool:
    st = bot["state"]["live"]
    if not st["pending"]:
        return False
    client, seq = _account()
    changed, keep = False, []
    for p in st["pending"]:
        try:
            o = normalize(client.request("GET", f"/api/v1/orders/{p['id']}", account_seq=seq) or {})
            ex = o.get("execution") or {}
            filled = ex.get("filledQuantity") or 0.0
            delta = filled - p.get("synced", 0.0)
            if delta > 0 and ex.get("averageFilledPrice"):
                _apply_fill(st, p["symbol"], p["side"], delta, ex["averageFilledPrice"])
                p["synced"] = filled
                changed = True
                bots.log(bot, "fill", f"[리밸런싱] {p['symbol']} {'매수' if p['side'] == 'BUY' else '매도'} 체결 {delta:g}주 @ {ex['averageFilledPrice']:,.2f}")
            if o.get("status") in TERMINAL:
                if o.get("status") == "REJECTED":
                    bots.log(bot, "error", f"[리밸런싱] {p['symbol']} 주문 거부됨 ({p['side']})", "error")
                changed = True
                continue
            keep.append(p)
        except ApiError as e:
            bots.log(bot, "error", f"체결 확인 실패: {e.message}", "warn")
            keep.append(p)
    st["pending"] = keep
    st["last_reconcile"] = now_iso()
    return changed


def _wait_sell_fills(bot: dict, today: str, sell_ids: set[str]) -> None:
    st = bot["state"]["live"]
    for i in range(SELL_SETTLE_POLLS):
        if i and SELL_SETTLE_INTERVAL > 0:
            time.sleep(SELL_SETTLE_INTERVAL)
        reconcile_live(bot, today)
        if not any(p["id"] in sell_ids for p in st["pending"]):
            return


def _send_market(bot: dict, symbol: str, side: str, qty: int, est_price: float, cid: str) -> dict:
    payload = {"clientOrderId": cid, "symbol": symbol, "side": side, "orderType": "MARKET",
               "timeInForce": "DAY", "quantity": str(qty)}
    built = {"market": bot["market"], "payload": payload, "checks": {"estimated_value": qty * est_price}}
    return trading.submit(built, bot["user_id"], source=f"bot:{bot['id']}")


def run_live(bot: dict, today: str) -> dict[str, Any]:
    st = bot["state"]["live"]
    if st["last_run_date"] == today:
        return {"skipped": "already_ran"}
    reconcile_live(bot, today)
    loaded = load(bot, today)
    _, _, today_bars, _, _ = loaded
    if len(today_bars) < len(bot["assets"]):
        raise ApiError(409, "market_not_open", "일부 종목의 오늘 시가가 아직 없습니다")

    d = decide(bot, today, loaded)
    # Mark day done before sending orders so a crash never double-orders.
    st["last_run_date"] = today
    bots.save(bot)

    client, seq = _account()
    m = bot["market"]
    currency = "KRW" if m == "KR" else "USD"
    syms = [a["symbol"] for a in bot["assets"]]
    rows = client.request("GET", "/api/v1/prices", params={"symbols": ",".join(syms)}) or []
    live_prices = {r["symbol"]: float(r["lastPrice"]) for r in rows if r.get("symbol") and r.get("lastPrice") is not None}
    prices = {s: live_prices.get(s, d["last_prices"][s]) for s in syms}

    positions = st.setdefault("positions", {})
    c = _costs(Market(m))
    headroom = max((c.slippage_pct + c.commission_pct) / 100.0, LIVE_BUY_HEADROOM)

    if d["due"]:
        tgt_w = d["target_weights_raw"]
        pos_val = sum(int(positions.get(s, {}).get("qty") or 0) * prices[s] for s in syms)
        target_cap = max(float(bot["capital"]), pos_val)
        desired = {
            s: (math.floor(target_cap * tgt_w.get(s, 0.0) / (prices[s] * (1.0 + headroom))) if tgt_w.get(s, 0.0) > 0 and prices[s] > 0 else 0)
            for s in syms
        }

        sent_sells: set[str] = set()
        n_sent = 0
        # 1) Send all SELL orders first
        for s in syms:
            cur_q = int(positions.get(s, {}).get("qty") or 0)
            diff = desired[s] - cur_q
            if diff < 0:
                try:
                    sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": s},
                                                     account_seq=seq) or {}).get("sellableQuantity") or 0)
                    qty = int(min(-diff, cur_q, sellable))
                    if qty <= 0:
                        continue
                    r = _send_market(bot, s, "SELL", qty, prices[s], _client_order_id(bot, today, s, "SELL"))
                    st["pending"].append({"kind": "order", "id": r["orderId"], "symbol": s, "side": "SELL",
                                          "qty": qty, "date": today, "synced": 0.0})
                    sent_sells.add(r["orderId"])
                    n_sent += 1
                    bots.log(bot, "order", f"[리밸런싱] {s} 매도 시장가 {qty}주")
                except ApiError as e:
                    bots.log(bot, "error", f"[리밸런싱] {s} 매도 실패: {e.message}", "error", {"code": e.code})

        has_buys = any(desired[s] > int(positions.get(s, {}).get("qty") or 0) for s in syms)
        if sent_sells and has_buys:
            _wait_sell_fills(bot, today, sent_sells)

        # 2) Send BUY orders within actual cash buying power & bot capital ceiling
        if has_buys:
            bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
            avail_cash = float(bp.get("cashBuyingPower") or 0.0)
            invested = sum(int(positions.get(s, {}).get("qty") or 0) * prices[s] for s in syms)
            bot_budget = max(float(bot["capital"]) - invested, 0.0)
            budget = min(avail_cash, bot_budget)
            for s in syms:
                cur_q = int(positions.get(s, {}).get("qty") or 0)
                diff = desired[s] - cur_q
                if diff <= 0:
                    continue
                unit = prices[s] * (1.0 + headroom)
                qty = min(diff, math.floor(budget / unit)) if unit > 0 else 0
                if qty <= 0:
                    bots.log(bot, "skip", f"[리밸런싱] {s} 자금 부족으로 매수 생략", "warn")
                    continue
                try:
                    r = _send_market(bot, s, "BUY", qty, prices[s], _client_order_id(bot, today, s, "BUY"))
                    st["pending"].append({"kind": "order", "id": r["orderId"], "symbol": s, "side": "BUY",
                                          "qty": qty, "date": today, "synced": 0.0})
                    budget = max(budget - qty * unit, 0.0)
                    n_sent += 1
                    bots.log(bot, "order", f"[리밸런싱] {s} 매수 시장가 {qty}주")
                except ApiError as e:
                    bots.log(bot, "error", f"[리밸런싱] {s} 매수 실패: {e.message}", "error", {"code": e.code})

        st["last_rebalance_date"] = today
        st["rebalances"].append({
            "date": today, "reason": d["reason"], "trades": n_sent, "weights": d["target_weights"],
        })
    else:
        bots.log(bot, "decision", f"{d['bar_date']} 기준 리밸런싱 조건 미해당")

    st["last_prices"] = prices
    st["target_weights"] = d["target_weights"]
    unreal = sum(int(p.get("qty") or 0) * (prices.get(s, p["entry_price"]) - p["entry_price"]) for s, p in positions.items())
    st["equity"].append({"date": today, "value": float(bot["capital"]) + st.get("realized", 0.0) + unreal})
    protections.evaluate_after_step(bot, st, today)
    bots.save(bot)
    return {"ran": today, "rebalanced": d["due"]}


def run(bot: dict, today: str) -> dict[str, Any]:
    return run_paper(bot, today) if bot["mode"] == "paper" else run_live(bot, today)
