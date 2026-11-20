from __future__ import annotations

import os
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent


def _bool(name: str, default: bool = False) -> bool:
    return os.getenv(name, str(default)).strip().lower() in {"1", "true", "yes", "on"}


def _csv(name: str) -> list[str]:
    return [v.strip().lower() for v in os.getenv(name, "").split(",") if v.strip()]


class Config:
    SECRET_KEY = os.getenv("SECRET_KEY", "dev-insecure-secret-do-not-use-in-prod")
    ENCRYPTION_KEY = os.getenv("ENCRYPTION_KEY", "")

    GOOGLE_CLIENT_ID = os.getenv("GOOGLE_CLIENT_ID", "")
    ALLOWED_EMAILS = _csv("ALLOWED_EMAILS")
    # Service admins (comma separated). Admins are auto-approved and manage sign-ups.
    # Kept in the environment (not the DB) so nobody can grant themselves admin by editing data.
    ADMIN_EMAILS = _csv("ADMIN_EMAILS")

    OTP_ISSUER = os.getenv("OTP_ISSUER", "MightyTrade")
    OTP_MAX_FAILS = int(os.getenv("OTP_MAX_FAILS", "5"))
    OTP_LOCK_MIN = int(os.getenv("OTP_LOCK_MIN", "5"))

    ACCESS_TOKEN_TTL_MIN = int(os.getenv("ACCESS_TOKEN_TTL_MIN", "720"))
    PENDING_TOKEN_TTL_MIN = int(os.getenv("PENDING_TOKEN_TTL_MIN", "10"))
    COOKIE_SECURE = _bool("COOKIE_SECURE", False)

    STORAGE_BACKEND = os.getenv("STORAGE_BACKEND", "json")
    DATA_DIR = Path(os.getenv("DATA_DIR", str(BASE_DIR / "data"))).resolve()

    MARKET_PROVIDERS = {
        "KR": os.getenv("MARKET_PROVIDER_KR", "mock"),
        "US": os.getenv("MARKET_PROVIDER_US", "mock"),
    }

    TOSS_API_BASE_URL = os.getenv("TOSS_API_BASE_URL", "https://openapi.tossinvest.com")
    # Toss keys are per account (registered in the app, stored encrypted), not in env.
    TOSS_TIMEOUT_SEC = float(os.getenv("TOSS_TIMEOUT_SEC", "10"))

    # Trading safety limits (per user)
    TRADING_UNLOCK_MIN = int(os.getenv("TRADING_UNLOCK_MIN", "30"))  # OTP unlock lasts this long
    TRADING_MAX_ORDERS_PER_DAY = int(os.getenv("TRADING_MAX_ORDERS_PER_DAY", "50"))
    TRADING_MAX_ORDER_KRW = float(os.getenv("TRADING_MAX_ORDER_KRW", "50000000"))  # per order
    TRADING_MAX_ORDER_USD = float(os.getenv("TRADING_MAX_ORDER_USD", "50000"))
    # Auto-trading scheduler (one per server; see app/autotrade/scheduler.py)
    AUTOTRADE_SCHEDULER = _bool("AUTOTRADE_SCHEDULER", True)


class DevelopmentConfig(Config):
    DEBUG = True


class ProductionConfig(Config):
    DEBUG = False
    COOKIE_SECURE = _bool("COOKIE_SECURE", True)


class TestingConfig(Config):
    TESTING = True
    AUTOTRADE_SCHEDULER = False


def get_config(name: str | None = None) -> type[Config]:
    name = name or os.getenv("FLASK_ENV", "development")
    return {
        "production": ProductionConfig,
        "testing": TestingConfig,
    }.get(name, DevelopmentConfig)
