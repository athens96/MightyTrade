"""Bot records and their event log (store collections `bots`, `bot_events`)."""
from __future__ import annotations

import re
import uuid
from datetime import datetime, timezone
from typing import Any, Optional

from ..errors import ApiError
from ..market.models import Market
from ..quant.strategies import STRATEGIES, StrategyParamError
from ..storage import get_store
from ..users.repository import now_iso

BOTS = "bots"
EVENTS = "bot_events"
MAX_BOTS = 20
MAX_EVENTS_PER_BOT = 300
SYMBOL_RE = {"KR": re.compile(r"[0-9A-Z]{6}"), "US": re.compile(r"[A-Z][A-Z0-9.\-]{0,11}")}


def empty_state(capital: float) -> dict:
    return {"position": {"qty": 0, "entry_price": 0.0, "entry_date": None}, "cash": capital, "pending": [],
            "trades": [], "equity": [], "last_run_date": None, "last_price": None, "last_reconcile": None}


def validate(body: dict, existing: Optional[dict] = None) -> dict:
    src = {**(existing or {}), **body}
    try:
        market = Market(str(src.get("market", "")).upper()).value
    except ValueError:
        raise ApiError(400, "invalid_market")
    symbol = str(src.get("symbol", "")).strip().upper()
    if not SYMBOL_RE[market].fullmatch(symbol):
        raise ApiError(400, "invalid_symbol")
    cls = STRATEGIES.get(src.get("strategy"))
    if cls is None:
        raise ApiError(400, "unknown_strategy")
    try:
        params = cls(src.get("params") or {}).p
    except StrategyParamError as e:
        raise ApiError(400, "invalid_params", str(e))
    try:
        capital = float(src.get("capital"))
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_capital", "운용 자금을 숫자로 입력하세요")
    if capital <= 0:
        raise ApiError(400, "invalid_capital", "운용 자금은 0보다 커야 합니다")
    name = str(src.get("name") or f"{cls.name} · {symbol}")[:60]
    return {"name": name, "market": market, "symbol": symbol, "strategy": cls.id, "params": params, "capital": capital}


def create(uid: str, body: dict) -> dict:
    if len(list_for(uid)) >= MAX_BOTS:
        raise ApiError(400, "too_many_bots", f"봇은 최대 {MAX_BOTS}개까지 만들 수 있습니다")
    fields = validate(body)
    bot_id = uuid.uuid4().hex[:12]
    rec = {**fields, "user_id": uid, "mode": "paper", "enabled": False, "created_at": now_iso(), "updated_at": now_iso(),
           "state": {"paper": empty_state(fields["capital"]), "live": empty_state(fields["capital"])}}
    return get_store().put(BOTS, bot_id, rec)


def get(uid: str, bot_id: str) -> dict:
    rec = get_store().get(BOTS, bot_id)
    if not rec or rec.get("user_id") != uid:
        raise ApiError(404, "bot_not_found")
    return rec


def save(bot: dict) -> dict:
    bot["updated_at"] = now_iso()
    return get_store().put(BOTS, bot["id"], bot)


def list_for(uid: str) -> list:
    return sorted(get_store().find(BOTS, lambda b: b.get("user_id") == uid), key=lambda b: b["created_at"])


def all_enabled() -> list:
    return get_store().find(BOTS, lambda b: b.get("enabled"))


def state(bot: dict) -> dict:
    return bot["state"][bot["mode"]]


def log(bot: dict, kind: str, message: str, level: str = "info", data: Optional[dict] = None) -> None:
    store = get_store()
    store.put(EVENTS, uuid.uuid4().hex, {
        "bot_id": bot["id"], "user_id": bot["user_id"], "mode": bot["mode"], "at": datetime.now(timezone.utc).isoformat(),
        "kind": kind, "level": level, "message": message, "data": data or {},
    })
    events = store.find(EVENTS, lambda e: e.get("bot_id") == bot["id"])
    if len(events) > MAX_EVENTS_PER_BOT:
        for e in sorted(events, key=lambda e: e["at"])[: len(events) - MAX_EVENTS_PER_BOT]:
            store.delete(EVENTS, e["id"])


def events(bot: dict, limit: int = 100) -> list:
    rows = get_store().find(EVENTS, lambda e: e.get("bot_id") == bot["id"])
    return sorted(rows, key=lambda e: e["at"], reverse=True)[:limit]


def public(bot: dict) -> dict:
    out = {k: v for k, v in bot.items() if k != "state"}
    st = state(bot)
    pos = st["position"]
    price = st.get("last_price") or pos.get("entry_price") or 0.0
    out["state"] = {
        "position": pos,
        "pending": st["pending"],
        "last_run_date": st["last_run_date"],
        "last_price": st.get("last_price"),
        "trades": st["trades"][-50:],
        "equity": st["equity"][-400:],
        **({"cash": st["cash"], "equity_now": st["cash"] + pos["qty"] * price} if bot["mode"] == "paper" else {}),
    }
    return out
