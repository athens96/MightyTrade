"""Backtest inputs: price history and cost defaults (real Toss commission when available)."""
from __future__ import annotations

import logging
from datetime import date

from flask import current_app, g

from ..errors import ApiError
from ..market.models import Market

log = logging.getLogger(__name__)


def cost_defaults(market: Market) -> dict:
    kr = market == Market.KR
    out = {
        "capital": 10_000_000 if kr else 10_000,
        "currency": "KRW" if kr else "USD",
        "commission_pct": 0.015,
        "commission_source": "default",
        # KR securities transaction tax on sells. The rate changes by year/market (KOSPI vs KOSDAQ):
        # verify the current rate and adjust in the form.
        "tax_pct": 0.20 if kr else 0.0,
        "slippage_pct": 0.05,
    }
    if current_app.config["MARKET_PROVIDERS"][market.value] != "toss":
        return out
    try:
        from ..integrations.toss import get_toss_client
        from ..users import credentials

        creds = credentials.get_toss(g.user["id"])
        if not creds or creds.get("account_seq") is None:
            return out
        rows = get_toss_client().request("GET", "/api/v1/commissions", account_seq=creds["account_seq"]) or []
        today = date.today().isoformat()
        for r in rows:
            current = (not r.get("startDate") or r["startDate"] <= today) and (not r.get("endDate") or r["endDate"] >= today)
            if r.get("marketCountry") == market.value and current:
                out["commission_pct"] = float(r["commissionRate"]) * 100
                out["commission_source"] = "toss"
                break
    except (ApiError, KeyError, ValueError, TypeError) as e:
        log.info("commission lookup failed, using default: %s", e)
    return out
