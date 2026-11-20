"""Fast public market-data pipeline for large-universe Quant Stock Screening (100~200+ stocks).

Why separate from TossProvider?
Toss Open API enforces tight per-user rate limits designed for live quotes, orderbooks, and
orders. Scanning 100~200 stocks directly through Toss `/candles` + `/investor-trading` triggers
HTTP 429 throttling and timeouts. Instead, the screener fetches public daily OHLCV, KR investor
flows, and KR market-cap rankings from Naver Finance (KR) and Yahoo Finance (US) with caching,
and reserves Toss API calls solely for final safety checks (warnings) on top finalists.
"""
from __future__ import annotations

import calendar
import re
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from typing import Any, Optional

import requests

from .engine import Bars

USER_AGENT = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
HTTP_TIMEOUT = 6
BARS_TTL = 600          # 10 min cache for daily candles
FLOW_TTL = 600          # 10 min cache for KR investor flows
LIST_TTL = 6 * 3600     # 6 hour cache for dynamic KR market-cap lists
MAX_BARS_CACHE = 800

_lock = threading.Lock()
_bars_cache: dict[tuple[str, str, int], tuple[float, Bars]] = {}
_flow_cache: dict[str, tuple[float, dict[str, Any]]] = {}
_list_cache: dict[str, tuple[float, list[tuple[str, str]]]] = {}

_PREF_SUFFIX_RE = re.compile(r"(우[A-Z0-9]?|우B|스팩\d*호?|리츠|인프라)$")
_INT_RE = re.compile(r"[+-]?\d+")
_FLOAT_RE = re.compile(r"[+-]?\d+(?:\.\d+)?")
_FCHART_ROW_RE = re.compile(r'data="(\d{8})\|([^"]+)"')


def _clean_int(raw: Any) -> Optional[int]:
    if raw is None:
        return None
    if isinstance(raw, (int, float)):
        return int(raw)
    s = str(raw).replace(",", "").strip()
    m = _INT_RE.search(s)
    return int(m.group(0)) if m else None


def _clean_float(raw: Any) -> Optional[float]:
    if raw is None:
        return None
    if isinstance(raw, (int, float)):
        return float(raw)
    s = str(raw).replace(",", "").strip()
    m = _FLOAT_RE.search(s)
    return float(m.group(0)) if m else None


def _is_ordinary_kr_stock(item: dict[str, Any]) -> bool:
    code = str(item.get("itemCode") or "").strip()
    name = str(item.get("stockName") or "").strip()
    end_type = str(item.get("stockEndType") or "stock").lower()
    if len(code) != 6 or not code.isdigit():
        return False
    if end_type != "stock":
        return False
    # Exclude preferred shares (e.g. 005935 삼성전자우, 005387 현대차2우B) and SPACs
    if code[-1] != "0":
        return False
    if _PREF_SUFFIX_RE.search(name) or "스팩" in name:
        return False
    return True


def fetch_kr_market_leaders(board: str, limit: int = 120) -> list[tuple[str, str]]:
    """Fetch live top ordinary stocks by market cap from Naver Finance ('KOSPI' | 'KOSDAQ')."""
    key = f"{board}:{limit}"
    now = time.time()
    with _lock:
        hit = _list_cache.get(key)
        if hit and hit[0] > now:
            return list(hit[1])

    out: list[tuple[str, str]] = []
    seen: set[str] = set()
    # Fetch up to 3 pages (100/page) so after filtering ETFs/preferred shares we still reach `limit`
    for page in range(1, 4):
        if len(out) >= limit:
            break
        try:
            r = requests.get(
                f"https://m.stock.naver.com/api/stocks/marketValue/{board}",
                params={"page": page, "pageSize": 100},
                headers={"User-Agent": USER_AGENT},
                timeout=HTTP_TIMEOUT,
            )
            if not r.ok:
                break
            stocks = r.json().get("stocks") or []
            if not stocks:
                break
            for st in stocks:
                if not _is_ordinary_kr_stock(st):
                    continue
                code = str(st["itemCode"]).strip()
                name = str(st["stockName"]).strip()
                if code not in seen:
                    seen.add(code)
                    out.append((code, name))
                    if len(out) >= limit:
                        break
        except Exception:
            break

    if out:
        with _lock:
            _list_cache[key] = (now + LIST_TTL, list(out))
    return out


# ------------------------------------------------------------------ daily OHLCV (public fast)
def _fetch_kr_bars_one(symbol: str, count: int) -> Optional[Bars]:
    try:
        r = requests.get(
            "https://fchart.stock.naver.com/sise.nhn",
            params={"symbol": symbol, "timeframe": "day", "count": str(count), "requestType": "0"},
            headers={"User-Agent": USER_AGENT},
            timeout=HTTP_TIMEOUT,
        )
        if not r.ok or not r.content:
            return None
        text = r.content.decode("cp949", errors="ignore")
        times, opens, highs, lows, closes, vols = [], [], [], [], [], []
        for ymd, rest in _FCHART_ROW_RE.findall(text):
            parts = rest.split("|")
            if len(parts) < 5:
                continue
            dt = datetime.strptime(ymd, "%Y%m%d")
            o, h, lo, c, v = float(parts[0]), float(parts[1]), float(parts[2]), float(parts[3]), float(parts[4])
            if c <= 0:
                continue
            # Naver occasionally emits 0 for open/high/low on thin/suspended bars: fall back to close
            o = o if o > 0 else c
            h = h if h > 0 else max(o, c)
            lo = lo if lo > 0 else min(o, c)
            times.append(calendar.timegm(dt.timetuple()))
            opens.append(o)
            highs.append(h)
            lows.append(lo)
            closes.append(c)
            vols.append(v)
        if len(times) < 20:
            return None
        return Bars(times, opens, highs, lows, closes, vols)
    except Exception:
        return None


def _fetch_us_bars_one(symbol: str, range_str: str) -> Optional[Bars]:
    # Yahoo uses '-' instead of '.' for class shares (e.g. BRK.B -> BRK-B)
    ysym = symbol.replace(".", "-")
    try:
        r = requests.get(
            f"https://query1.finance.yahoo.com/v8/finance/chart/{ysym}",
            params={"range": range_str, "interval": "1d"},
            headers={"User-Agent": USER_AGENT},
            timeout=HTTP_TIMEOUT,
        )
        if not r.ok:
            return None
        res_list = (r.json().get("chart") or {}).get("result") or []
        if not res_list:
            return None
        res = res_list[0]
        raw_t = res.get("timestamp") or []
        quote = ((res.get("indicators") or {}).get("quote") or [{}])[0]
        raw_o = quote.get("open") or []
        raw_h = quote.get("high") or []
        raw_l = quote.get("low") or []
        raw_c = quote.get("close") or []
        raw_v = quote.get("volume") or []
        adj_list = (res.get("indicators") or {}).get("adjclose") or []
        raw_adj = adj_list[0].get("adjclose") if adj_list else None

        times, opens, highs, lows, closes, vols = [], [], [], [], [], []
        for idx, t in enumerate(raw_t):
            c = raw_c[idx] if idx < len(raw_c) else None
            if c is None or c <= 0:
                continue
            o = raw_o[idx] if idx < len(raw_o) and raw_o[idx] else c
            h = raw_h[idx] if idx < len(raw_h) and raw_h[idx] else max(o, c)
            lo = raw_l[idx] if idx < len(raw_l) and raw_l[idx] else min(o, c)
            v = raw_v[idx] if idx < len(raw_v) and raw_v[idx] is not None else 0.0
            # Apply split/dividend adjustment ratio smoothly when adjclose is present
            if raw_adj and idx < len(raw_adj) and raw_adj[idx] and raw_adj[idx] > 0:
                ratio = raw_adj[idx] / c
                o, h, lo, c = o * ratio, h * ratio, lo * ratio, raw_adj[idx]
            times.append(int(t))
            opens.append(float(o))
            highs.append(float(h))
            lows.append(float(lo))
            closes.append(float(c))
            vols.append(float(v))
        if len(times) < 20:
            return None
        return Bars(times, opens, highs, lows, closes, vols)
    except Exception:
        return None


def fetch_public_bars(market: str, symbols: list[str], *, count: int = 320) -> dict[str, Bars]:
    """Batch-fetch daily OHLCV bars for screening (cached 10m, 12 concurrent workers)."""
    now = time.time()
    bucket = 520 if count > 350 else 320
    range_str = "2y" if bucket > 350 else "18mo"
    out: dict[str, Bars] = {}
    missing: list[str] = []

    with _lock:
        for sym in symbols:
            hit = _bars_cache.get((market, sym, bucket))
            if hit and hit[0] > now:
                out[sym] = hit[1]
            else:
                missing.append(sym)

    if missing:
        def _worker(sym: str) -> tuple[str, Optional[Bars]]:
            b = _fetch_kr_bars_one(sym, bucket) if market == "KR" else _fetch_us_bars_one(sym, range_str)
            return sym, b

        workers = min(12, len(missing))
        with ThreadPoolExecutor(max_workers=workers) as pool:
            fetched = list(pool.map(_worker, missing))

        with _lock:
            if len(_bars_cache) > MAX_BARS_CACHE:
                _bars_cache.clear()
            for sym, b in fetched:
                if b is not None:
                    _bars_cache[(market, sym, bucket)] = (now + BARS_TTL, b)
                    out[sym] = b

    return out


# ------------------------------------------------------------------ KR investor flows (public fast)
def _fetch_kr_flow_one(symbol: str) -> tuple[str, Optional[dict[str, Any]]]:
    try:
        r = requests.get(
            f"https://m.stock.naver.com/api/stock/{symbol}/trend",
            params={"pageSize": 20, "page": 1},
            headers={"User-Agent": USER_AGENT},
            timeout=HTTP_TIMEOUT,
        )
        if not r.ok:
            return symbol, None
        recs = r.json()
        if not isinstance(recs, list) or not recs:
            return symbol, None
        f5 = sum(_clean_int(x.get("foreignerPureBuyQuant")) or 0 for x in recs[:5])
        i5 = sum(_clean_int(x.get("organPureBuyQuant")) or 0 for x in recs[:5])
        f20 = sum(_clean_int(x.get("foreignerPureBuyQuant")) or 0 for x in recs[:20])
        i20 = sum(_clean_int(x.get("organPureBuyQuant")) or 0 for x in recs[:20])
        h_now = _clean_float(recs[0].get("foreignerHoldRatio"))
        h_old = _clean_float(recs[-1].get("foreignerHoldRatio")) if len(recs) >= 2 else None
        f_delta = round(h_now - h_old, 2) if (h_now is not None and h_old is not None) else None
        return symbol, {
            "flow_5d_for": f5,
            "flow_5d_inst": i5,
            "flow_20d_for": f20,
            "flow_20d_inst": i20,
            "foreign_delta_pct": f_delta,
        }
    except Exception:
        return symbol, None


def fetch_kr_flows(symbols: list[str]) -> dict[str, dict[str, Any]]:
    """Batch-fetch 20-day KR foreign & institution net buying from Naver Finance (cached 10m)."""
    now = time.time()
    out: dict[str, dict[str, Any]] = {}
    missing: list[str] = []
    with _lock:
        for sym in symbols:
            hit = _flow_cache.get(sym)
            if hit and hit[0] > now:
                out[sym] = hit[1]
            else:
                missing.append(sym)

    if missing:
        workers = min(12, len(missing))
        with ThreadPoolExecutor(max_workers=workers) as pool:
            for sym, row in pool.map(_fetch_kr_flow_one, missing):
                if row is not None:
                    with _lock:
                        _flow_cache[sym] = (now + FLOW_TTL, row)
                    out[sym] = row
    return out
