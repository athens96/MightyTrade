"""Unified multi-bot control tower analytics & Paper-vs-Live execution scorecard (MT-9)."""
from __future__ import annotations

from typing import Any, Optional

from . import bots, semi

USD_KRW_REF = 1380.0

REBAL_LABELS = {
    "daily": "매일",
    "weekly": "매주",
    "biweekly": "격주(2주)",
    "monthly": "매월 초",
    "quarterly": "분기 초",
    "yearly": "매년 초",
    "drift": "비중 편차 이탈 시",
}
WEEKDAY_LABELS = ["월요일", "화요일", "수요일", "목요일", "금요일"]


def _fx(market: str) -> float:
    return USD_KRW_REF if market == "US" else 1.0


def _state_equity_and_invested(bot: dict, ledger: str) -> tuple[float, float]:
    """Return (current_equity, invested_value) in native currency for the given ledger ('paper' or 'live')."""
    st = bot["state"][ledger]
    cap = float(bot["capital"])
    kind = bots.bot_kind(bot)
    if kind in ("rebalance", "rotation"):
        positions = st.get("positions") or {}
        last_prices = st.get("last_prices") or {}
        pos_val = sum(
            int(p.get("qty") or 0) * float(last_prices.get(sym, p.get("entry_price") or 0.0))
            for sym, p in positions.items()
        )
        if ledger == "paper":
            eq = float(st.get("cash", cap)) + pos_val
        else:
            eq = float(st["equity"][-1]["value"]) if st.get("equity") else (cap + float(st.get("realized") or 0.0))
        return eq, pos_val

    pos = st["position"]
    px = float(st.get("last_price") or pos.get("entry_price") or 0.0)
    pos_val = float(pos["qty"]) * px
    if ledger == "paper":
        eq = float(st.get("cash", cap)) + pos_val
    else:
        eq = (
            float(st["equity"][-1]["value"])
            if st.get("equity")
            else (cap + float(st.get("realized") or 0.0) + float(pos["qty"]) * (px - float(pos["entry_price"])))
        )
    return eq, pos_val


def _ledger_trade_stats(bot: dict, ledger: str) -> dict[str, Any]:
    st = bot["state"][ledger]
    cap = float(bot["capital"])
    has_run = bool(st.get("last_run_date") or st.get("equity") or st.get("trades") or st.get("rebalances"))
    eq, _ = _state_equity_and_invested(bot, ledger)
    ret_pct = ((eq / cap - 1.0) * 100.0) if (cap > 0 and has_run) else None

    kind = bots.bot_kind(bot)
    if kind in ("rebalance", "rotation"):
        rebs = st.get("rebalances") or []
        return {
            "has_run": has_run,
            "return_pct": round(ret_pct, 2) if ret_pct is not None else None,
            "trades": len(rebs),
            "win_rate_pct": None,
        }

    closed = [t for t in (st.get("trades") or []) if t.get("exit_date") is not None]
    wins = sum(1 for t in closed if (t.get("pnl") or 0.0) > 0)
    win_rate = (wins / len(closed) * 100.0) if closed else None
    trade_cnt = len(closed) if kind != "dca" else int(st.get("dca_steps") or len(st.get("trades") or []))
    return {
        "has_run": has_run,
        "return_pct": round(ret_pct, 2) if ret_pct is not None else None,
        "trades": trade_cnt,
        "win_rate_pct": round(win_rate, 1) if win_rate is not None else None,
    }


def _summarize_group(group: list[dict], ledger: str) -> dict[str, Any]:
    by_market: dict[str, dict[str, Any]] = {}
    for m in ("KR", "US"):
        sub = [b for b in group if b["market"] == m]
        cap = sum(float(b["capital"]) for b in sub)
        eq_inv = [_state_equity_and_invested(b, ledger) for b in sub]
        eq = sum(x[0] for x in eq_inv)
        inv = sum(x[1] for x in eq_inv)
        pnl = eq - cap
        by_market[m] = {
            "bots": len(sub),
            "enabled": sum(1 for b in sub if b.get("enabled")),
            "capital": round(cap, 2),
            "equity": round(eq, 2),
            "invested": round(inv, 2),
            "pnl": round(pnl, 2),
            "return_pct": round((pnl / cap * 100.0), 2) if cap > 0 else 0.0,
        }

    cap_krw = sum(by_market[m]["capital"] * _fx(m) for m in ("KR", "US"))
    eq_krw = sum(by_market[m]["equity"] * _fx(m) for m in ("KR", "US"))
    inv_krw = sum(by_market[m]["invested"] * _fx(m) for m in ("KR", "US"))
    pnl_krw = eq_krw - cap_krw
    kr_eq_krw = by_market["KR"]["equity"]
    us_eq_krw = by_market["US"]["equity"] * USD_KRW_REF
    tot_eq_denom = kr_eq_krw + us_eq_krw

    return {
        "bots": len(group),
        "enabled": sum(1 for b in group if b.get("enabled")),
        "capital_krw": round(cap_krw),
        "equity_krw": round(eq_krw),
        "invested_krw": round(inv_krw),
        "pnl_krw": round(pnl_krw),
        "return_pct": round((pnl_krw / cap_krw * 100.0), 2) if cap_krw > 0 else 0.0,
        "kr_share_pct": round((kr_eq_krw / tot_eq_denom * 100.0), 1) if tot_eq_denom > 0 else 0.0,
        "us_share_pct": round((us_eq_krw / tot_eq_denom * 100.0), 1) if tot_eq_denom > 0 else 0.0,
        "by_market": by_market,
    }


def _bot_symbols(bot: dict) -> list[str]:
    kind = bots.bot_kind(bot)
    if kind == "rebalance":
        return [str(a["symbol"]).upper() for a in (bot.get("assets") or []) if a.get("symbol")]
    if kind == "rotation":
        st = bots.state(bot)
        held = [s for s, p in (st.get("positions") or {}).items() if int(p.get("qty") or 0) > 0]
        picks = [str(x["symbol"]).upper() for x in (st.get("selected_picks") or []) if x.get("symbol")]
        return list(dict.fromkeys(held + picks))
    sym = str(bot.get("symbol") or "").strip().upper()
    return [sym] if sym and ":" not in sym and "," not in sym else []


def _next_schedule_label(bot: dict) -> str:
    kind = bots.bot_kind(bot)
    if kind == "webhook":
        return "외부 웹훅 신호 수신 시 즉시 실행"
    if kind == "dca":
        cad = REBAL_LABELS.get(str(bot.get("rebalance") or "weekly"), "매주")
        if bot.get("rebalance") in ("weekly", "biweekly"):
            wd = WEEKDAY_LABELS[int(bot.get("weekday") or 0) % 5]
            return f"{cad} {wd} 장 시작 1분 뒤 적립"
        return f"{cad} 장 시작 1분 뒤 적립"
    if kind == "rotation":
        cad = REBAL_LABELS.get(str(bot.get("rebalance") or "weekly"), "매주")
        return f"{cad} 상위 {bot.get('top_n', 5)}종목 순환 점검"
    if kind == "rebalance":
        cad = REBAL_LABELS.get(str(bot.get("rebalance") or "monthly"), "매월 초")
        return f"{cad} 비중 리밸런싱 점검"
    tf = bots.bot_timeframe(bot)
    if tf != "1d":
        return f"장중 {tf} 봉 마감마다 · 마감 10분 전 청산"
    return "매일 장 시작 1분 뒤 일봉 신호 실행"


def _combined_return_series(user_bots: list[dict]) -> list[dict[str, Any]]:
    paper_bots = [b for b in user_bots if b["mode"] == "paper"]
    live_bots = [b for b in user_bots if b["mode"] in ("live", "semi")]

    all_dates: set[str] = set()
    for b in paper_bots:
        all_dates.update(pt["date"] for pt in (b["state"]["paper"].get("equity") or []))
    for b in live_bots:
        all_dates.update(pt["date"] for pt in (b["state"]["live"].get("equity") or []))

    if not all_dates:
        return []

    sorted_dates = sorted(all_dates)[-180:]
    paper_maps = [{pt["date"]: float(pt["value"]) for pt in (b["state"]["paper"].get("equity") or [])} for b in paper_bots]
    live_maps = [{pt["date"]: float(pt["value"]) for pt in (b["state"]["live"].get("equity") or [])} for b in live_bots]

    paper_last = [float(b["capital"]) for b in paper_bots]
    live_last = [float(b["capital"]) for b in live_bots]
    paper_cap_krw = sum(float(b["capital"]) * _fx(b["market"]) for b in paper_bots)
    live_cap_krw = sum(float(b["capital"]) * _fx(b["market"]) for b in live_bots)

    out: list[dict[str, Any]] = []
    for d in sorted_dates:
        for i, mp in enumerate(paper_maps):
            if d in mp:
                paper_last[i] = mp[d]
        for i, mp in enumerate(live_maps):
            if d in mp:
                live_last[i] = mp[d]
        p_eq = sum(paper_last[i] * _fx(paper_bots[i]["market"]) for i in range(len(paper_bots)))
        l_eq = sum(live_last[i] * _fx(live_bots[i]["market"]) for i in range(len(live_bots)))
        out.append({
            "date": d,
            "paper_return_pct": round(((p_eq / paper_cap_krw - 1.0) * 100.0), 2) if paper_cap_krw > 0 else None,
            "live_return_pct": round(((l_eq / live_cap_krw - 1.0) * 100.0), 2) if live_cap_krw > 0 else None,
        })
    return out


def build_overview(uid: str) -> dict[str, Any]:
    user_bots = bots.list_for(uid)
    paper_group = [b for b in user_bots if b["mode"] == "paper"]
    live_group = [b for b in user_bots if b["mode"] in ("live", "semi")]

    # Duplicate symbol detection across bots
    sym_buckets: dict[tuple[str, str], list[dict]] = {}
    for b in user_bots:
        for sym in _bot_symbols(b):
            sym_buckets.setdefault((b["market"], sym), []).append(b)

    duplicate_symbols = []
    for (mkt, sym), shared in sorted(sym_buckets.items()):
        if len(shared) < 2:
            continue
        live_active = sum(1 for b in shared if b["mode"] in ("live", "semi") and b.get("enabled"))
        duplicate_symbols.append({
            "market": mkt,
            "symbol": sym,
            "live_conflict": live_active >= 2,
            "bots": [
                {
                    "id": b["id"],
                    "name": b["name"],
                    "mode": b["mode"],
                    "enabled": bool(b.get("enabled")),
                    "category": bots.bot_category(b),
                }
                for b in shared
            ],
        })

    # Upcoming schedule + active alerts
    upcoming = []
    pending_approvals_count = 0
    tripped_count = 0
    for b in sorted(user_bots, key=lambda x: (not bool(x.get("enabled")), x["created_at"])):
        st = bots.state(b)
        prop = semi.active_proposal(st)
        if prop:
            pending_approvals_count += 1
        if st.get("tripped"):
            tripped_count += 1
        upcoming.append({
            "bot_id": b["id"],
            "name": b["name"],
            "market": b["market"],
            "category": bots.bot_category(b),
            "mode": b["mode"],
            "enabled": bool(b.get("enabled")),
            "schedule": _next_schedule_label(b),
            "last_run_date": st.get("last_run_date"),
            "pending_approval_orders": len(prop["orders"]) if prop else 0,
            "tripped": bool(st.get("tripped")),
        })

    # Execution quality & Paper <-> Live scorecard
    all_bps: list[float] = []
    scorecard = []
    for b in user_bots:
        p_stats = _ledger_trade_stats(b, "paper")
        l_stats = _ledger_trade_stats(b, "live")
        fills = b["state"]["live"].get("fills") or []
        bot_bps = [float(f["slippage_bps"]) for f in fills if f.get("slippage_bps") is not None]
        all_bps.extend(bot_bps)
        avg_bps: Optional[float] = round(sum(bot_bps) / len(bot_bps), 2) if bot_bps else None
        gap_pct: Optional[float] = (
            round(l_stats["return_pct"] - p_stats["return_pct"], 2)
            if (l_stats["return_pct"] is not None and p_stats["return_pct"] is not None)
            else None
        )
        scorecard.append({
            "bot_id": b["id"],
            "name": b["name"],
            "market": b["market"],
            "category": bots.bot_category(b),
            "mode": b["mode"],
            "enabled": bool(b.get("enabled")),
            "paper_return_pct": p_stats["return_pct"],
            "paper_trades": p_stats["trades"],
            "paper_win_rate_pct": p_stats["win_rate_pct"],
            "live_return_pct": l_stats["return_pct"],
            "live_trades": l_stats["trades"],
            "live_win_rate_pct": l_stats["win_rate_pct"],
            "return_gap_pct": gap_pct,
            "fills_count": len(fills),
            "avg_slippage_bps": avg_bps,
        })

    return {
        "total_bots": len(user_bots),
        "enabled_bots": sum(1 for b in user_bots if b.get("enabled")),
        "pending_approvals_count": pending_approvals_count,
        "tripped_count": tripped_count,
        "usd_krw_ref": USD_KRW_REF,
        "paper": _summarize_group(paper_group, "paper"),
        "live": _summarize_group(live_group, "live"),
        "combined_equity": _combined_return_series(user_bots),
        "duplicate_symbols": duplicate_symbols,
        "upcoming": upcoming,
        "execution": {
            "total_fills": len(all_bps),
            "avg_slippage_bps": round(sum(all_bps) / len(all_bps), 2) if all_bps else None,
            "items": scorecard,
        },
    }
