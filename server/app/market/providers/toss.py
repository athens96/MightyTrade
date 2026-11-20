from __future__ import annotations

import calendar
import re
import threading
import time
from datetime import datetime
from typing import Any, Optional

from flask import g

from ...errors import ApiError
from ...integrations.toss import get_toss_client
from ..aggregate import resample_intraday, resample_weekly
from ..models import Candle, Instrument, Interval, Market, Quote
from .base import MarketDataProvider

PAGE_SIZE = 200          # Toss max candles per call
MAX_PAGES = 10           # caps 1m-derived bars (e.g. 1h ≈ 2000 min of trading)
CANDLE_TTL = {"1m": 10, "1d": 60}   # seconds, in-process cache
STOCK_LIST_TTL = 12 * 3600
SEARCH_LIMIT = 30

# Toss has no market param: the symbol decides the market. Validate so KR/US tabs can't mix.
SYMBOL_RE = {Market.KR: re.compile(r"[0-9A-Z]{6}"), Market.US: re.compile(r"[A-Z][A-Z0-9.\-]{0,11}")}
STOCK_MARKETS = {Market.KR: ["KOSPI", "KOSDAQ", "KR_ETC"], Market.US: ["NASDAQ", "NYSE", "AMEX", "US_ETC"]}


def _ts(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def _to_candle(raw: dict, base: str) -> Candle:
    dt = _ts(raw["timestamp"])
    # Daily bars: Toss stamps them at local midnight (KR 00:00 KST, US 00:00 ET = 14:00 KST).
    # Normalize to UTC midnight of that trading date so charts show the right day.
    t = calendar.timegm(dt.date().timetuple()) if base == "1d" else int(dt.timestamp())
    return Candle(
        time=t,
        open=float(raw["openPrice"]),
        high=float(raw["highPrice"]),
        low=float(raw["lowPrice"]),
        close=float(raw["closePrice"]),
        volume=float(raw["volume"]),
    )


def _uid() -> str:
    return g.user["id"]


class TossProvider(MarketDataProvider):
    """Toss Securities Open API market data. Candles: 1m/1d native, 5m/15m/1h/1w resampled.

    Every call uses the logged-in user's own Toss key, and caches are per user: Toss terms forbid
    redistributing data, so data fetched with one account's key is never served to another.
    """

    name = "toss"

    def __init__(self, config: dict):
        super().__init__(config)
        self._lock = threading.Lock()
        self._candle_cache: dict[tuple, tuple[float, list[Candle]]] = {}
        self._stocks: dict[tuple[str, Market], tuple[float, list[Instrument]]] = {}

    # ------------------------------------------------------------ helpers
    def _check(self, market: Market, symbol: str) -> None:
        if not SYMBOL_RE[market].fullmatch(symbol):
            raise ApiError(400, "symbol_market_mismatch", f"{symbol} is not a {market.value} symbol")

    def _fetch_base(self, symbol: str, base: str, needed: int) -> list[Candle]:
        """Fetch `needed` most recent 1m/1d candles (ascending), paging with nextBefore."""
        key = (_uid(), symbol, base, needed)
        now = time.time()
        with self._lock:
            hit = self._candle_cache.get(key)
            if hit and hit[0] > now:
                return hit[1]

        client = get_toss_client()
        out: list[Candle] = []
        before: Optional[str] = None
        for _ in range(MAX_PAGES):
            params: dict[str, Any] = {"symbol": symbol, "interval": base, "count": min(PAGE_SIZE, needed - len(out))}
            if before:
                params["before"] = before
            result = client.request("GET", "/api/v1/candles", params=params) or {}
            out.extend(_to_candle(c, base) for c in result.get("candles", []))  # newest first
            before = result.get("nextBefore")
            if not before or len(out) >= needed:
                break

        candles = sorted({c.time: c for c in out}.values(), key=lambda c: c.time)
        with self._lock:
            self._candle_cache = {k: v for k, v in self._candle_cache.items() if v[0] > now}
            self._candle_cache[key] = (now + CANDLE_TTL[base], candles)
        return candles

    # ---------------------------------------------------------- interface
    def get_candles(self, market: Market, symbol: str, interval: Interval, limit: int = 200) -> list[Candle]:
        self._check(market, symbol)
        if interval in (Interval.M1, Interval.D1):
            return self._fetch_base(symbol, interval.value, limit)[-limit:]
        if interval == Interval.W1:
            daily = self._fetch_base(symbol, "1d", min((limit + 1) * 5, PAGE_SIZE * MAX_PAGES))
            return resample_weekly(daily)[-limit:]
        factor = interval.seconds // 60
        minutes = self._fetch_base(symbol, "1m", min((limit + 1) * factor, PAGE_SIZE * MAX_PAGES))
        return resample_intraday(minutes, interval.seconds)[-limit:]

    def get_quote(self, market: Market, symbol: str) -> Quote:
        self._check(market, symbol)
        prices = get_toss_client().request("GET", "/api/v1/prices", params={"symbols": symbol}) or []
        if not prices:
            raise ApiError(404, "stock_not_found", symbol)
        p = prices[0]
        price = float(p["lastPrice"])
        daily = self._fetch_base(symbol, "1d", 2)
        # Latest daily bar is the current/last session; change is vs the session before it.
        prev = daily[-2].close if len(daily) >= 2 else price
        change = price - prev
        ts = int(_ts(p["timestamp"]).timestamp()) if p.get("timestamp") else int(time.time())
        return Quote(
            market=market, symbol=symbol, price=price, change=round(change, 4),
            change_pct=round(change / prev * 100, 2) if prev else 0.0,
            currency=p.get("currency") or ("KRW" if market == Market.KR else "USD"), time=ts,
        )

    def _stock_list(self, market: Market) -> list[Instrument]:
        now = time.time()
        with self._lock:
            hit = self._stocks.get((_uid(), market))
            if hit and hit[0] > now:
                return hit[1]
        client = get_toss_client()
        items: list[Instrument] = []
        for i, m in enumerate(STOCK_MARKETS[market]):
            if i:
                time.sleep(1.05)  # STOCK_ALL rate limit: 1 req/s
            for s in client.request("GET", "/api/v1/stocks/all", params={"market": m, "status": "ACTIVE"}) or []:
                items.append(Instrument(market, s["symbol"], s.get("name") or s.get("englishName") or "",
                                        s.get("market", m), s.get("englishName") or ""))
        with self._lock:
            self._stocks = {k: v for k, v in self._stocks.items() if v[0] > now}
            self._stocks[(_uid(), market)] = (now + STOCK_LIST_TTL, items)
        return items

    def search(self, market: Market, query: str) -> list[Instrument]:
        """Toss has no search API: match against the cached full stock list."""
        q = query.strip().lower()
        if not q:
            return []
        scored = []
        for inst in self._stock_list(market):
            sym, name = inst.symbol.lower(), inst.name.lower()
            eng = inst.english_name.lower()
            if sym == q:
                rank = 0
            elif sym.startswith(q):
                rank = 1
            elif name.startswith(q) or eng.startswith(q):
                rank = 2
            elif q in name or q in eng:
                rank = 3
            else:
                continue
            scored.append((rank, len(inst.name), inst))
        scored.sort(key=lambda x: (x[0], x[1]))
        return [inst for _, _, inst in scored[:SEARCH_LIMIT]]
