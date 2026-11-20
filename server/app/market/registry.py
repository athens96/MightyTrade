from __future__ import annotations

from flask import current_app

from .models import Market
from .providers.base import MarketDataProvider
from .providers.investing import InvestingProvider
from .providers.mock import MockProvider
from .providers.toss import TossProvider

PROVIDERS: dict[str, type[MarketDataProvider]] = {
    "mock": MockProvider,
    "toss": TossProvider,
    "investing": InvestingProvider,
}


def get_provider(market: Market) -> MarketDataProvider:
    """Provider per market is chosen by MARKET_PROVIDER_KR / MARKET_PROVIDER_US."""
    cache = current_app.extensions.setdefault("market_providers", {})
    name = current_app.config["MARKET_PROVIDERS"][market.value]
    if name not in cache:
        if name not in PROVIDERS:
            raise ValueError(f"Unknown market provider: {name}")
        cache[name] = PROVIDERS[name](current_app.config)
    return cache[name]
