"""Public fundamental metrics (PER, PBR, ROE, dividend yield) for quant stock screening.

Sources (zero API key required, 6-hour in-memory cache):
- US live: Yahoo Finance batch quote API (1 request for up to 40 symbols).
- KR live: Naver Finance mobile JSON summary endpoint (concurrent fetch).
- Mock mode / tests: deterministic offline synthetic fundamentals.
"""
from __future__ import annotations

import hashlib
import re
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Optional

import requests

FUNDAMENTALS_TTL = 6 * 3600
CRUMB_TTL = 1800
HTTP_TIMEOUT = 5
USER_AGENT = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"

_lock = threading.Lock()
_cache: dict[tuple[str, str], tuple[float, dict[str, Any]]] = {}
_yahoo_state: dict[str, Any] = {"session": None, "crumb": None, "expires": 0.0}

_NUM_RE = re.compile(r"-?\d+(?:\.\d+)?")


def _parse_float(raw: Any) -> Optional[float]:
    if raw is None:
        return None
    if isinstance(raw, (int, float)):
        return float(raw)
    s = str(raw).replace(",", "").strip()
    if not s or s in ("-", "N/A", "적자", "없음"):
        return None
    m = _NUM_RE.search(s)
    return float(m.group(0)) if m else None


def _parse_kr_market_cap(raw: Any) -> Optional[float]:
    """Parse Naver Korean market cap string like '1,057조 5,918억' into KRW."""
    if not raw:
        return None
    s = str(raw).replace(",", "").strip()
    total = 0.0
    found = False
    m_jo = re.search(r"(\d+(?:\.\d+)?)\s*조", s)
    if m_jo:
        total += float(m_jo.group(1)) * 1e12
        found = True
    m_eok = re.search(r"(\d+(?:\.\d+)?)\s*억", s)
    if m_eok:
        total += float(m_eok.group(1)) * 1e8
        found = True
    return total if found else None


def _empty(source: str) -> dict[str, Any]:
    return {"per": None, "pbr": None, "roe": None, "dividend_yield": None, "market_cap": None, "source": source}


def _mock_fundamental(market: str, symbol: str) -> dict[str, Any]:
    """Deterministic realistic fundamentals for mock/offline testing."""
    h = int(hashlib.sha256(f"{market}:{symbol}:fund".encode()).hexdigest()[:12], 16)
    per = round(6.0 + (h % 320) / 10.0, 2)             # 6.0 ~ 37.9
    pbr = round(0.5 + ((h >> 10) % 75) / 10.0, 2)      # 0.5 ~ 7.9
    roe = round(4.0 + ((h >> 18) % 360) / 10.0, 2)     # 4.0 ~ 39.9%
    div = round(((h >> 26) % 62) / 10.0, 2)            # 0.0 ~ 6.1%
    mcap = float((5 + ((h >> 34) % 450)) * (1e12 if market == "KR" else 1e9))
    return {"per": per, "pbr": pbr, "roe": roe, "dividend_yield": div, "market_cap": mcap, "source": "mock"}


def _yahoo_session() -> tuple[requests.Session, str]:
    now = time.time()
    with _lock:
        if _yahoo_state["session"] is not None and _yahoo_state["crumb"] and _yahoo_state["expires"] > now:
            return _yahoo_state["session"], _yahoo_state["crumb"]

    sess = requests.Session()
    sess.headers["User-Agent"] = USER_AGENT
    sess.get("https://fc.yahoo.com", timeout=HTTP_TIMEOUT)
    r = sess.get("https://query1.finance.yahoo.com/v1/test/getcrumb", timeout=HTTP_TIMEOUT)
    r.raise_for_status()
    crumb = r.text.strip()
    if not crumb:
        raise RuntimeError("empty yahoo crumb")

    with _lock:
        _yahoo_state["session"] = sess
        _yahoo_state["crumb"] = crumb
        _yahoo_state["expires"] = now + CRUMB_TTL
    return sess, crumb


def _fetch_us_yahoo(symbols: list[str]) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    if not symbols:
        return out
    try:
        sess, crumb = _yahoo_session()
    except Exception:
        return {s: _empty("unavailable") for s in symbols}

    for i in range(0, len(symbols), 40):
        batch = symbols[i : i + 40]
        try:
            r = sess.get(
                "https://query1.finance.yahoo.com/v7/finance/quote",
                params={"symbols": ",".join(batch), "crumb": crumb},
                timeout=HTTP_TIMEOUT,
            )
            if r.status_code in (401, 403):
                with _lock:
                    _yahoo_state["expires"] = 0.0
                sess, crumb = _yahoo_session()
                r = sess.get(
                    "https://query1.finance.yahoo.com/v7/finance/quote",
                    params={"symbols": ",".join(batch), "crumb": crumb},
                    timeout=HTTP_TIMEOUT,
                )
            if not r.ok:
                continue
            rows = (r.json().get("quoteResponse") or {}).get("result") or []
            for q in rows:
                sym = str(q.get("symbol") or "").upper()
                per = _parse_float(q.get("trailingPE"))
                pbr = _parse_float(q.get("priceToBook"))
                eps = _parse_float(q.get("epsTrailingTwelveMonths"))
                bv = _parse_float(q.get("bookValue"))
                roe = round(eps / bv * 100.0, 2) if (eps is not None and bv is not None and bv > 0) else None
                raw_div = _parse_float(q.get("trailingAnnualDividendYield"))
                if raw_div is not None:
                    div = round(raw_div * 100.0, 2)
                else:
                    alt_div = _parse_float(q.get("dividendYield"))
                    div = round(alt_div if (alt_div is not None and alt_div > 0.5) else (alt_div * 100.0), 2) if alt_div is not None else None
                mcap = _parse_float(q.get("marketCap"))
                out[sym] = {
                    "per": round(per, 2) if per is not None and per > 0 else None,
                    "pbr": round(pbr, 2) if pbr is not None and pbr > 0 else None,
                    "roe": roe,
                    "dividend_yield": div if div is not None and div >= 0 else None,
                    "market_cap": mcap,
                    "source": "yahoo",
                }
        except Exception:
            continue

    for s in symbols:
        out.setdefault(s, _empty("unavailable"))
    return out


def _fetch_kr_one(symbol: str) -> tuple[str, dict[str, Any]]:
    try:
        r = requests.get(
            f"https://m.stock.naver.com/api/stock/{symbol}/integration",
            headers={"User-Agent": USER_AGENT},
            timeout=HTTP_TIMEOUT,
        )
        if not r.ok:
            return symbol, _empty("unavailable")
        data = r.json()
        infos = {it.get("code"): it.get("value") for it in (data.get("totalInfos") or []) if isinstance(it, dict)}
        per = _parse_float(infos.get("per"))
        pbr = _parse_float(infos.get("pbr"))
        eps = _parse_float(infos.get("eps"))
        bps = _parse_float(infos.get("bps"))
        div = _parse_float(infos.get("dividendYieldRatio"))
        mcap = _parse_kr_market_cap(infos.get("marketValue"))
        roe = round(eps / bps * 100.0, 2) if (eps is not None and bps is not None and bps > 0) else None
        return symbol, {
            "per": round(per, 2) if per is not None and per > 0 else None,
            "pbr": round(pbr, 2) if pbr is not None and pbr > 0 else None,
            "roe": roe,
            "dividend_yield": round(div, 2) if div is not None and div >= 0 else None,
            "market_cap": mcap,
            "source": "naver",
        }
    except Exception:
        return symbol, _empty("unavailable")


def _fetch_kr_naver(symbols: list[str]) -> dict[str, dict[str, Any]]:
    if not symbols:
        return {}
    out: dict[str, dict[str, Any]] = {}
    with ThreadPoolExecutor(max_workers=min(6, len(symbols))) as pool:
        for sym, row in pool.map(_fetch_kr_one, symbols):
            out[sym] = row
    return out


def get_fundamentals(market: str, symbols: list[str], *, use_mock: bool = False) -> dict[str, dict[str, Any]]:
    """Fetch fundamental metrics for `symbols` in `market` ('KR'|'US'), with 6h in-memory caching."""
    clean = [s.strip().upper() for s in symbols if s and s.strip()]
    if use_mock:
        return {s: _mock_fundamental(market, s) for s in clean}

    now = time.time()
    result: dict[str, dict[str, Any]] = {}
    missing: list[str] = []
    with _lock:
        for s in clean:
            hit = _cache.get((market, s))
            if hit and hit[0] > now:
                result[s] = hit[1]
            else:
                missing.append(s)

    if missing:
        fetched = _fetch_kr_naver(missing) if market == "KR" else _fetch_us_yahoo(missing)
        with _lock:
            for s, row in fetched.items():
                ttl = FUNDAMENTALS_TTL if row.get("source") != "unavailable" else 300
                _cache[(market, s)] = (now + ttl, row)
                result[s] = row

    return result
