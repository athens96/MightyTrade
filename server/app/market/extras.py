"""Toss-only market data: indices, session status, investor warnings, investor flows, orderbook.

Everything is fetched with the logged-in user's own key and cached per user (Toss terms: no redistribution).
"""
from __future__ import annotations

import threading
import time
from datetime import datetime, timezone
from typing import Any, Callable

from flask import current_app, g

from ..account.toss_account import SYMBOL_RE, normalize
from ..errors import ApiError
from ..integrations.toss import get_toss_client
from .models import Market

INDICES = ("KOSPI", "KOSDAQ")
SESSION_LABELS = {
    "KR": {"preMarket": "장전 거래 (NXT)", "regularMarket": "정규장", "afterMarket": "장후 거래 (NXT)"},
    "US": {"dayMarket": "데이마켓", "preMarket": "프리마켓", "regularMarket": "정규장", "afterMarket": "애프터마켓"},
}
WARNING_LABELS = {
    "LIQUIDATION_TRADING": ("정리매매", "critical"),
    "OVERHEATED": ("단기과열", "warning"),
    "INVESTMENT_WARNING": ("투자경고", "serious"),
    "INVESTMENT_RISK": ("투자위험", "critical"),
    "VI_STATIC_AND_DYNAMIC": ("VI 발동 (정적·동적)", "serious"),
    "VI_STATIC": ("VI 발동 (정적)", "serious"),
    "VI_DYNAMIC": ("VI 발동 (동적)", "serious"),
    "STOCK_WARRANTS": ("신주인수권", "warning"),
}

_cache: dict = {}
_lock = threading.Lock()


def _cached(key: tuple, ttl: float, fn: Callable[[], Any]) -> Any:
    now = time.time()
    with _lock:
        hit = _cache.get(key)
        if hit and hit[0] > now:
            return hit[1]
    value = fn()
    with _lock:
        for k in [k for k, v in _cache.items() if v[0] <= now]:
            _cache.pop(k, None)
        _cache[key] = (now + ttl, value)
    return value


def _require_toss(market: Market) -> None:
    if current_app.config["MARKET_PROVIDERS"][market.value] != "toss":
        raise ApiError(501, "requires_toss", "토스증권 시세 연동에서만 제공됩니다")


def _check_symbol(market: Market, symbol: str) -> None:
    if not SYMBOL_RE[market.value].fullmatch(symbol):
        raise ApiError(400, "symbol_market_mismatch", f"{symbol} is not a {market.value} symbol")


def _ts(s: str) -> datetime:
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


# ------------------------------------------------------------------ session status
def sessions_from_calendar(market: str, cal: dict) -> list:
    """Flatten previous/today/next business day into [(name, start, end)] (US sessions cross midnight KST)."""
    out = []
    for day in ("previousBusinessDay", "today", "nextBusinessDay"):
        d = cal.get(day) or {}
        block = d.get("integrated") if market == "KR" else d
        if not isinstance(block, dict):
            continue
        for name in SESSION_LABELS[market]:
            s = block.get(name)
            if isinstance(s, dict) and s.get("startTime") and s.get("endTime"):
                out.append((name, _ts(s["startTime"]), _ts(s["endTime"])))
    return sorted(out, key=lambda x: x[1])


def session_status(market: str, cal: dict, now: datetime) -> dict:
    sessions = sessions_from_calendar(market, cal)
    labels = SESSION_LABELS[market]
    for name, start, end in sessions:
        if start <= now < end:
            return {"open": True, "session": name, "label": labels[name], "until": end.isoformat()}
    upcoming = next(((n, s) for n, s, _ in sessions if s > now), None)
    today_open = isinstance((cal.get("today") or {}).get("integrated" if market == "KR" else "regularMarket"), dict)
    return {
        "open": False,
        "session": None,
        "label": "장 마감" if today_open else "휴장",
        "next": {"session": upcoming[0], "label": labels[upcoming[0]], "at": upcoming[1].isoformat()} if upcoming else None,
    }


# ------------------------------------------------------------------ public functions
def overview() -> dict:
    """KOSPI/KOSDAQ with change vs previous close, and KR/US session status."""
    uid = g.user["id"]
    _require_toss(Market.KR)

    def load_indices():
        client = get_toss_client()
        rows = {r["symbol"]: r for r in client.request("GET", "/api/v1/market-indicators/prices",
                                                       params={"symbols": ",".join(INDICES)}) or []}
        out = []
        for sym in INDICES:
            if sym not in rows:
                continue
            price = float(rows[sym]["lastPrice"])
            daily = (client.request("GET", f"/api/v1/market-indicators/{sym}/candles",
                                    params={"interval": "1d", "count": 2}) or {}).get("candles", [])
            prev = float(daily[1]["closePrice"]) if len(daily) >= 2 else price
            out.append({"symbol": sym, "price": price, "change": price - prev,
                        "change_pct": (price / prev - 1) * 100 if prev else 0.0})
        return out

    now = datetime.now(timezone.utc)
    indices = _cached((uid, "indices"), 15, load_indices)
    return {"indices": indices, "status": {m: market_status(m, now) for m in ("KR", "US")}}


def calendar(market: str) -> dict:
    """Toss market calendar for the logged-in user (cached 10 min)."""
    now = datetime.now(timezone.utc)
    return _cached((g.user["id"], "calendar", market, now.date().isoformat()), 600,
                   lambda: get_toss_client().request("GET", f"/api/v1/market-calendar/{market}") or {})


def market_status(market: str, now: datetime | None = None) -> dict:
    return session_status(market, calendar(market), now or datetime.now(timezone.utc))


def regular_session(market: str) -> tuple | None:
    """(trading date, regular start, regular end) for today's regular session, or None on holidays."""
    cal = calendar(market).get("today") or {}
    block = cal.get("integrated") if market == "KR" else cal
    reg = (block or {}).get("regularMarket") if isinstance(block, dict) else None
    if not isinstance(reg, dict):
        return None
    return cal.get("date"), _ts(reg["startTime"]), _ts(reg["endTime"])


def warnings(market: Market, symbol: str) -> list:
    _require_toss(market)
    _check_symbol(market, symbol)

    def load():
        rows = get_toss_client().request("GET", f"/api/v1/stocks/{symbol}/warnings") or []
        return [{"type": r["warningType"], "label": WARNING_LABELS.get(r["warningType"], (r["warningType"], "warning"))[0],
                 "level": WARNING_LABELS.get(r["warningType"], ("", "warning"))[1],
                 "start": r.get("startDate"), "end": r.get("endDate")} for r in rows]
    return _cached((g.user["id"], "warnings", symbol), 60, load)


def investor_flow(symbol: str, count: int) -> dict:
    """Daily net buying by individuals / foreigners / institutions (KR only), newest first."""
    _require_toss(Market.KR)
    _check_symbol(Market.KR, symbol)

    def load():
        r = get_toss_client().request("GET", f"/api/v1/stocks/{symbol}/investor-trading", params={"count": count}) or {}
        out = []
        for rec in normalize(r.get("records", [])):
            out.append({
                "date": rec["date"],
                "individual": (rec.get("individual") or {}).get("netBuyVolume"),
                "foreigner": (rec.get("foreigner") or {}).get("netBuyVolume"),
                "institution": (rec.get("institution") or {}).get("netBuyVolume"),
                # Toss holdingRate is a fraction (0.459 = 45.9%)
                "foreign_holding_pct": (None if (rec.get("foreignerHolding") or {}).get("holdingRate") is None
                                        else rec["foreignerHolding"]["holdingRate"] * 100),
            })
        return out
    return {"records": _cached((g.user["id"], "flow", symbol, count), 300, load)}


def orderbook(market: Market, symbol: str) -> dict:
    """Snapshot to draw immediately; live updates come from the realtime stream."""
    _require_toss(market)
    _check_symbol(market, symbol)
    client = get_toss_client()
    book = normalize(client.request("GET", "/api/v1/orderbook", params={"symbol": symbol}) or {})
    trades = normalize(client.request("GET", "/api/v1/trades", params={"symbol": symbol, "count": 30}) or [])
    return {"orderbook": book, "trades": trades}
