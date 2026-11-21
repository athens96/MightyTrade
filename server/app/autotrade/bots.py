"""Bot records and their event log (store collections `bots`, `bot_events`)."""
from __future__ import annotations

import re
import uuid
from datetime import datetime, timezone
from typing import Any, Optional

from ..errors import ApiError
from ..market.models import Market
from ..quant import market_timing, portfolio
from ..quant.engine import RiskRules
from ..quant.strategies import STRATEGIES, StrategyParamError
from ..storage import get_store
from ..users.repository import now_iso
from . import protections

BOTS = "bots"
EVENTS = "bot_events"
MAX_BOTS = 20
MAX_EVENTS_PER_BOT = 300
SYMBOL_RE = {"KR": re.compile(r"[0-9A-Z]{6}"), "US": re.compile(r"[A-Z][A-Z0-9.\-]{0,11}")}


def bot_kind(bot: dict) -> str:
    return bot.get("kind") or "single"


def bot_category(bot: dict) -> str:
    k = bot_kind(bot)
    if k in ("rebalance", "rotation", "dca"):
        return k
    cls = STRATEGIES.get(bot.get("strategy", ""))
    return getattr(cls, "category", "swing") if cls else "swing"


def bot_timeframe(bot: dict) -> str:
    if bot_kind(bot) in ("rebalance", "rotation", "dca"):
        return "1d"
    cls = STRATEGIES.get(bot.get("strategy", ""))
    return getattr(cls, "timeframe", "1d") if cls else "1d"


def empty_state(capital: float) -> dict:
    return {"position": {"qty": 0, "entry_price": 0.0, "entry_date": None}, "cash": capital, "pending": [],
            "trades": [], "equity": [], "last_run_date": None, "last_price": None, "last_reconcile": None}


def empty_rebalance_state(capital: float) -> dict:
    return {"positions": {}, "cash": capital, "realized": 0.0, "pending": [], "rebalances": [],
            "trades": [], "equity": [], "last_run_date": None, "last_rebalance_date": None,
            "target_weights": {}, "last_prices": {}, "last_reconcile": None}


def has_open_position(bot: dict, mode: Optional[str] = None) -> bool:
    modes = (mode,) if mode else ("paper", "live")
    if bot_kind(bot) in ("rebalance", "rotation"):
        return any(
            any(int(p.get("qty") or 0) > 0 for p in (bot["state"][m].get("positions") or {}).values())
            for m in modes
        )
    return any(bot["state"][m]["position"]["qty"] > 0 for m in modes)


def validate(body: dict, existing: Optional[dict] = None) -> dict:
    src = {**(existing or {}), **body}
    kind = str(src.get("kind") or "single")
    if kind not in ("single", "rebalance", "rotation", "dca"):
        raise ApiError(400, "invalid_kind")
    try:
        market = Market(str(src.get("market", "")).upper()).value
    except ValueError:
        raise ApiError(400, "invalid_market")
    try:
        capital = float(src.get("capital"))
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_capital", "운용 자금을 숫자로 입력하세요")
    if capital <= 0:
        raise ApiError(400, "invalid_capital", "운용 자금은 0보다 커야 합니다")

    if kind == "rebalance":
        model = portfolio.PORTFOLIO_MODELS.get(src.get("model"))
        if model is None:
            raise ApiError(400, "unknown_model", "알 수 없는 리밸런싱 이론입니다")
        try:
            params = model.resolve(src.get("params") or {})
        except StrategyParamError as e:
            raise ApiError(400, "invalid_params", str(e))
        assets = portfolio.validate_assets(market, src.get("assets") or model.presets.get(market), model)
        rebalance = str(src.get("rebalance") or model.default_rebalance)
        if rebalance not in portfolio.REBALANCE_MODES:
            raise ApiError(400, "invalid_rebalance")
        try:
            drift_pct = float(src.get("drift_pct", 5.0))
        except (TypeError, ValueError):
            raise ApiError(400, "invalid_number", "허용 편차를 숫자로 입력하세요")
        if not 0.5 <= drift_pct <= 50.0:
            raise ApiError(400, "out_of_range", "허용 편차는 0.5 ~ 50% 범위여야 합니다")
        name = str(src.get("name") or f"{model.name} ({market})")[:60]
        guard = protections.validate_guard(src.get("guard"))
        return {
            "kind": "rebalance", "category": "rebalance", "timeframe": "1d",
            "name": name, "market": market, "symbol": ",".join(a["symbol"] for a in assets),
            "model": model.id, "assets": assets, "rebalance": rebalance, "drift_pct": drift_pct,
            "buy_only": bool(src.get("buy_only", False)),
            "params": params, "guard": guard, "capital": capital,
        }

    if kind == "dca":
        from . import dca
        symbol = str(src.get("symbol", "")).strip().upper()
        if not SYMBOL_RE[market].fullmatch(symbol):
            raise ApiError(400, "invalid_symbol")
        cadence = str(src.get("rebalance") or "weekly")
        if cadence not in dca.DCA_CADENCES:
            raise ApiError(400, "invalid_rebalance", "적립 주기는 daily / weekly / biweekly / monthly 중 하나여야 합니다")
        try:
            weekday = int(float(src.get("weekday", 0)))
            base_amount = float(src.get("base_amount") or (100_000 if market == "KR" else 100))
            dip_mult = float(src.get("dip_mult", 1.5))
            peak_mult = float(src.get("peak_mult", 0.5))
        except (TypeError, ValueError):
            raise ApiError(400, "invalid_number", "적립 금액·배수 설정값은 숫자여야 합니다")
        if not 0 <= weekday <= 4:
            raise ApiError(400, "out_of_range", "적립 요일은 0(월) ~ 4(금) 범위여야 합니다")
        if base_amount <= 0 or base_amount > capital:
            raise ApiError(400, "out_of_range", "1회 적립 금액은 0보다 크고 총 운용 자금 이하여야 합니다")
        if not (1.0 <= dip_mult <= 3.0 and 0.0 <= peak_mult <= 1.0):
            raise ApiError(400, "out_of_range", "세일 배수(1~3x) 및 과열 배수(0~1x) 범위를 확인하세요")
        guard = protections.validate_guard(src.get("guard"))
        mt_cfg = market_timing.validate_market_timing(src.get("market_timing"), market)
        name = str(src.get("name") or f"스마트 적립 · {symbol}")[:60]
        return {
            "kind": "dca", "category": "dca", "timeframe": "1d",
            "name": name, "market": market, "symbol": symbol, "rebalance": cadence, "weekday": weekday,
            "base_amount": round(base_amount, 2), "smart_scale": bool(src.get("smart_scale", True)),
            "dip_mult": round(dip_mult, 2), "peak_mult": round(peak_mult, 2),
            "params": {}, "guard": guard, "market_timing": mt_cfg, "capital": capital,
        }

    if kind == "rotation":
        from ..quant import screener
        from . import rotation
        preset_id = str(src.get("preset") or "momentum_quality")
        preset = screener.SCREENER_PRESETS.get(preset_id)
        if preset is None:
            raise ApiError(400, "unknown_preset", "알 수 없는 종목 발굴 방식입니다")
        if preset["kr_only"] and market != "KR":
            raise ApiError(400, "kr_only_preset", "이 발굴 방식은 국내(KR) 시장 전용입니다")
        universe = str(src.get("universe") or ("kr_all_150" if market == "KR" else "us_sp100"))
        if universe not in screener.UNIVERSES[market] and universe not in screener.UNIVERSE_ALIASES:
            raise ApiError(400, "unknown_universe", "알 수 없는 후보 종목 묶음입니다")
        custom_raw = src.get("custom_symbols")
        if isinstance(custom_raw, str):
            custom_syms = [s.strip().upper() for s in re.split(r"[\s,;]+", custom_raw) if s.strip()]
        elif isinstance(custom_raw, list):
            custom_syms = [str(s).strip().upper() for s in custom_raw if str(s).strip()]
        else:
            custom_syms = []
        if universe == "custom" and len(custom_syms) < 2:
            raise ApiError(400, "too_few_symbols", "직접 입력 종목은 최소 2개 이상이어야 합니다")
        try:
            top_n = int(float(src.get("top_n", 5)))
            hold_buffer = int(float(src.get("hold_buffer", max(top_n * 2, 10))))
        except (TypeError, ValueError):
            raise ApiError(400, "invalid_number", "상위 종목 수·유지 순위는 정수여야 합니다")
        if not 2 <= top_n <= 15:
            raise ApiError(400, "out_of_range", "상위 편입 종목 수는 2 ~ 15개 범위여야 합니다")
        if not top_n <= hold_buffer <= 40:
            raise ApiError(400, "out_of_range", f"유지 순위 버퍼는 {top_n} ~ 40위 범위여야 합니다")
        cadence = str(src.get("rebalance") or "weekly")
        if cadence not in rotation.ROTATION_CADENCES:
            raise ApiError(400, "invalid_rebalance", "순환 주기는 weekly / biweekly / monthly 중 하나여야 합니다")
        strat_id = str(src.get("strategy") or preset["recommended_strategy"])
        guard = protections.validate_guard(src.get("guard"))
        mt_cfg = market_timing.validate_market_timing(src.get("market_timing"), market)
        name = str(src.get("name") or f"순환매 · {preset['name']} ({market})")[:60]
        return {
            "kind": "rotation", "category": "rotation", "timeframe": "1d",
            "name": name, "market": market, "symbol": f"{universe}:TOP{top_n}",
            "preset": preset_id, "universe": universe, "custom_symbols": custom_syms,
            "strategy": strat_id, "top_n": top_n, "hold_buffer": hold_buffer,
            "rebalance": cadence, "exclude_warnings": bool(src.get("exclude_warnings", True)),
            "params": {}, "guard": guard, "market_timing": mt_cfg, "capital": capital,
        }

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
        risk = RiskRules.from_dict(src.get("risk")).to_dict()
    except (TypeError, ValueError) as e:
        raise ApiError(400, "invalid_risk", str(e))
    guard = protections.validate_guard(src.get("guard"))
    mt_cfg = market_timing.validate_market_timing(src.get("market_timing"), market)
    name = str(src.get("name") or f"{cls.name} · {symbol}")[:60]
    return {
        "kind": "single", "category": cls.category, "timeframe": cls.timeframe,
        "name": name, "market": market, "symbol": symbol, "strategy": cls.id, "params": params,
        "risk": risk, "guard": guard, "market_timing": mt_cfg, "capital": capital,
    }


def create(uid: str, body: dict) -> dict:
    if len(list_for(uid)) >= MAX_BOTS:
        raise ApiError(400, "too_many_bots", f"봇은 최대 {MAX_BOTS}개까지 만들 수 있습니다")
    fields = validate(body)
    bot_id = uuid.uuid4().hex[:12]
    init_state = empty_rebalance_state if fields["kind"] in ("rebalance", "rotation") else empty_state
    rec = {**fields, "user_id": uid, "mode": "paper", "enabled": False, "created_at": now_iso(), "updated_at": now_iso(),
           "state": {"paper": init_state(fields["capital"]), "live": init_state(fields["capital"])}}
    return get_store().put(BOTS, bot_id, rec)


def get(uid: str, bot_id: str) -> dict:
    rec = get_store().get(BOTS, bot_id)
    if not rec or rec.get("user_id") != uid:
        raise ApiError(404, "bot_not_found")
    return rec


def save(bot: dict) -> dict:
    bot["updated_at"] = now_iso()
    return get_store().put(BOTS, bot["id"], bot)


def delete(bot_id: str) -> None:
    """Delete a bot and all of its event log entries."""
    store = get_store()
    store.delete(BOTS, bot_id)
    for e in store.find(EVENTS, lambda r: r.get("bot_id") == bot_id):
        store.delete(EVENTS, e["id"])


def list_for(uid: str) -> list:
    return sorted(get_store().find(BOTS, lambda b: b.get("user_id") == uid), key=lambda b: b["created_at"])


def all_enabled() -> list:
    return get_store().find(BOTS, lambda b: b.get("enabled"))


def state_key(mode: str) -> str:
    return "paper" if mode == "paper" else "live"


def state(bot: dict) -> dict:
    return bot["state"][state_key(bot.get("mode", "paper"))]


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
    if kind in ("order", "approval", "fill", "error", "skip", "halt", "protection"):
        from . import notifier
        notifier.notify_bot_log(bot, kind, message, level)


def events(bot: dict, limit: int = 100) -> list:
    rows = get_store().find(EVENTS, lambda e: e.get("bot_id") == bot["id"])
    return sorted(rows, key=lambda e: e["at"], reverse=True)[:limit]


def public(bot: dict) -> dict:
    out = {k: v for k, v in bot.items() if k != "state"}
    out["kind"] = bot_kind(bot)
    out["category"] = bot_category(bot)
    out["timeframe"] = bot_timeframe(bot)
    st = state(bot)
    from . import semi
    pending_app = semi.active_proposal(st)
    if out["kind"] in ("rebalance", "rotation"):
        positions = st.get("positions") or {}
        last_prices = st.get("last_prices") or {}
        pos_val = sum(
            int(p.get("qty") or 0) * float(last_prices.get(sym, p.get("entry_price") or 0.0))
            for sym, p in positions.items()
        )
        out["state"] = {
            "position": {"qty": sum(int(p.get("qty") or 0) for p in positions.values()), "entry_price": 0.0, "entry_date": None},
            "positions": positions,
            "pending": st["pending"],
            "pending_approval": pending_app,
            "last_run_date": st["last_run_date"],
            "last_rebalance_date": st.get("last_rebalance_date"),
            "target_weights": st.get("target_weights") or {},
            "selected_picks": st.get("selected_picks") or [],
            "last_prices": last_prices,
            "last_price": pos_val,
            "rebalances": (st.get("rebalances") or [])[-30:],
            "trades": [],
            "equity": st["equity"][-400:],
            "tripped": st.get("tripped"),
            "cooldown_until": st.get("cooldown_until"),
            **({"cash": st["cash"], "equity_now": st["cash"] + pos_val} if bot["mode"] == "paper" else {}),
        }
        return out
    pos = st["position"]
    price = st.get("last_price") or pos.get("entry_price") or 0.0
    dca_extra = {}
    if out["kind"] == "dca":
        from . import dca
        dca_extra["dca_stats"] = dca.public_dca_stats(st)
    out["state"] = {
        "position": pos,
        "pending": st["pending"],
        "pending_approval": pending_app,
        "last_run_date": st["last_run_date"],
        "last_price": st.get("last_price"),
        "trades": st["trades"][-50:],
        "equity": st["equity"][-400:],
        "tripped": st.get("tripped"),
        "cooldown_until": st.get("cooldown_until"),
        **dca_extra,
        **({"cash": st["cash"], "equity_now": st["cash"] + pos["qty"] * price} if bot["mode"] == "paper" else {}),
    }
    return out
