"""Bot-level circuit breakers, consecutive-loss guards, cooldowns, and daily loss limits (MT-4)."""
from __future__ import annotations

from datetime import date, timedelta
from typing import Any, Optional

from ..errors import ApiError
from ..users.repository import now_iso

DEFAULT_GUARD: dict[str, Any] = {
    "max_drawdown_pct": 0.0,
    "max_consecutive_losses": 0,
    "cooldown_days": 0,
    "daily_loss_limit_pct": 0.0,
}


def validate_guard(raw: Optional[dict]) -> dict[str, Any]:
    if not raw or not isinstance(raw, dict):
        return dict(DEFAULT_GUARD)
    try:
        mdd = float(raw.get("max_drawdown_pct") or 0.0)
        streak = int(float(raw.get("max_consecutive_losses") or 0))
        cooldown = int(float(raw.get("cooldown_days") or 0))
        daily = float(raw.get("daily_loss_limit_pct") or 0.0)
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_guard", "서킷브레이커 설정값은 숫자여야 합니다")
    if not 0.0 <= mdd <= 80.0:
        raise ApiError(400, "invalid_guard", "최대 허용 낙폭은 0 ~ 80% 범위여야 합니다")
    if not 0 <= streak <= 20:
        raise ApiError(400, "invalid_guard", "연속 손절 한도는 0 ~ 20회 범위여야 합니다")
    if not 0 <= cooldown <= 60:
        raise ApiError(400, "invalid_guard", "재진입 쿨다운 일수는 0 ~ 60일 범위여야 합니다")
    if not 0.0 <= daily <= 50.0:
        raise ApiError(400, "invalid_guard", "일일 손실 한도는 0 ~ 50% 범위여야 합니다")
    return {
        "max_drawdown_pct": round(mdd, 4),
        "max_consecutive_losses": streak,
        "cooldown_days": cooldown,
        "daily_loss_limit_pct": round(daily, 4),
    }


def guard_of(bot: dict) -> dict[str, Any]:
    return {**DEFAULT_GUARD, **(bot.get("guard") or {})}


def acknowledge_on_enable(bot: dict) -> None:
    """Clear tripped lock when the user explicitly re-enables a bot, advancing baseline cursors."""
    for m in ("paper", "live"):
        st = bot["state"].get(m)
        if not st:
            continue
        if st.pop("tripped", None):
            eqs = st.get("equity") or []
            st["guard_reset_equity_idx"] = len(eqs)
            st["guard_reset_baseline"] = eqs[-1]["value"] if eqs else float(bot["capital"])
            st["guard_reset_trade_idx"] = len(st.get("trades") or [])


def check_buy_allowed(bot: dict, st: dict, today: str, current_equity: Optional[float] = None) -> Optional[str]:
    """Return human-readable block reason if new BUY entries should be suppressed today, else None."""
    if st.get("tripped"):
        return f"서킷브레이커 보호 잠금 중 ({st['tripped'].get('reason', '')})"
    g = guard_of(bot)
    cd_until = st.get("cooldown_until")
    if g["cooldown_days"] > 0 and cd_until and today <= cd_until:
        return f"손절 후 쿨다운 휴식 중 (~{cd_until})"
    if g["daily_loss_limit_pct"] > 0:
        eqs = st.get("equity") or []
        prev_eq = next((e["value"] for e in reversed(eqs) if e["date"] < today), None)
        if prev_eq is None and eqs:
            prev_eq = float(bot["capital"])
        cur = current_equity if current_equity is not None else (eqs[-1]["value"] if eqs else None)
        if prev_eq and prev_eq > 0 and cur is not None:
            drop_pct = (1.0 - cur / prev_eq) * 100.0
            if drop_pct >= g["daily_loss_limit_pct"]:
                return f"일일 손실 한도 도달 (-{drop_pct:.1f}% ≤ -{g['daily_loss_limit_pct']:g}%)"
    return None


def trip_bot(bot: dict, st: dict, today: str, reason: str) -> None:
    from . import bots, executor

    bot["enabled"] = False
    st["tripped"] = {"at": now_iso(), "date": today, "reason": reason}
    if bot.get("mode") in ("live", "semi") and st.get("pending"):
        try:
            executor.cancel_live_pending(bot)
        except Exception:  # noqa: BLE001
            pass
    st["pending"] = []
    bots.log(bot, "protection", f"🛑 서킷브레이커 발동 — 봇 자동 일시정지: {reason}", "warn")


def evaluate_after_step(bot: dict, st: dict, today: str) -> Optional[str]:
    """Update post-loss cooldown and trip circuit breaker if MDD or consecutive-loss limits breached."""
    g = guard_of(bot)

    # 1. Update post-loss cooldown if a new losing trade closed
    trades = st.get("trades") or []
    closed = [t for t in trades if t.get("exit_date") is not None and t.get("pnl") is not None]
    if g["cooldown_days"] > 0 and closed:
        last_t = closed[-1]
        if (last_t.get("pnl") or 0.0) < 0 and last_t["exit_date"] > (st.get("last_cooldown_exit") or ""):
            try:
                until = (date.fromisoformat(str(last_t["exit_date"])[:10]) + timedelta(days=g["cooldown_days"])).isoformat()
                st["cooldown_until"] = until
                st["last_cooldown_exit"] = last_t["exit_date"]
            except ValueError:
                pass

    # 2. Max drawdown circuit breaker
    eqs = st.get("equity") or []
    if g["max_drawdown_pct"] > 0 and eqs:
        reset_idx = int(st.get("guard_reset_equity_idx") or 0)
        window = eqs[reset_idx:]
        if window:
            baseline = float(bot["capital"]) if reset_idx == 0 else float(st.get("guard_reset_baseline") or bot["capital"])
            peak = max([baseline] + [float(e["value"]) for e in window])
            cur = float(window[-1]["value"])
            if peak > 0:
                dd_pct = (1.0 - cur / peak) * 100.0
                if dd_pct >= g["max_drawdown_pct"]:
                    reason = f"최대 허용 낙폭 도달 (-{dd_pct:.1f}% / 한도 -{g['max_drawdown_pct']:g}%)"
                    trip_bot(bot, st, today, reason)
                    return reason

    # 3. Consecutive losing trades circuit breaker
    if g["max_consecutive_losses"] > 0 and closed:
        reset_t_idx = int(st.get("guard_reset_trade_idx") or 0)
        recent_closed = [t for t in trades[reset_t_idx:] if t.get("exit_date") is not None and t.get("pnl") is not None]
        streak = 0
        for t in reversed(recent_closed):
            if (t.get("pnl") or 0.0) < 0:
                streak += 1
            else:
                break
        if streak >= g["max_consecutive_losses"]:
            reason = f"{streak}회 연속 손절 발생 (한도 {g['max_consecutive_losses']}회)"
            trip_bot(bot, st, today, reason)
            return reason

    return None
