"""Auto-trading API: bots, paper/live mode, kill switch."""
from __future__ import annotations

from datetime import datetime, timezone

from flask import Blueprint, current_app, g, jsonify, request

from ..auth import otp
from ..auth.decorators import login_required
from ..errors import ApiError
from ..users import credentials
from ..users import repository as users
from . import bots, executor
from .scheduler import active_session, target_date

bp = Blueprint("autotrade", __name__)


def _user() -> dict:
    return users.get_user(g.user["id"]) or g.user


def _need_toss() -> None:
    if not credentials.get_toss(g.user["id"]):
        raise ApiError(409, "toss_credentials_required", "토스증권 API 키를 먼저 등록하세요 (설정)")


@bp.get("/status")
@login_required
def status():
    sched = current_app.extensions.get("autotrade_scheduler")
    return jsonify(
        halted=bool(_user().get("autotrade_halted")),
        scheduler={"running": bool(sched and sched.running), "last_tick": sched.last_tick if sched else None,
                   "error": sched.last_error if sched else None},
    )


@bp.post("/halt")
@login_required
def halt():
    """Kill switch: stop all bots now and cancel their open Toss orders. Resuming needs an OTP."""
    users.update_user(g.user["id"], autotrade_halted=True)
    cancelled = 0
    for bot in bots.list_for(g.user["id"]):
        if bot["state"]["live"]["pending"]:
            try:
                cancelled += executor.cancel_live_pending(bot)
            except ApiError as e:
                bots.log(bot, "error", f"전체 중지 중 취소 실패: {e.message}", "error")
        bots.log(bot, "halt", "전체 중지 — 자동매매가 멈췄습니다", "warn")
    return jsonify(halted=True, cancelled=cancelled)


@bp.post("/resume")
@login_required
def resume():
    otp.verify(_user(), str((request.get_json(silent=True) or {}).get("otp_code", "")))
    users.update_user(g.user["id"], autotrade_halted=False)
    return jsonify(halted=False)


@bp.get("/bots")
@login_required
def list_bots():
    return jsonify(items=[bots.public(b) for b in bots.list_for(g.user["id"])])


@bp.post("/bots")
@login_required
def create_bot():
    return jsonify(bot=bots.public(bots.create(g.user["id"], request.get_json(silent=True) or {})))


@bp.get("/bots/<bot_id>")
@login_required
def get_bot(bot_id: str):
    bot = bots.get(g.user["id"], bot_id)
    return jsonify(bot=bots.public(bot), events=bots.events(bot, int(request.args.get("limit", 100))))


@bp.put("/bots/<bot_id>")
@login_required
def update_bot(bot_id: str):
    bot = bots.get(g.user["id"], bot_id)
    body = request.get_json(silent=True) or {}
    holding = any(bot["state"][m]["position"]["qty"] > 0 for m in ("paper", "live"))
    if holding and any(k in body and body[k] != bot.get(k) for k in ("market", "symbol", "strategy")):
        raise ApiError(409, "bot_has_position", "보유 중인 봇은 종목·전략을 바꿀 수 없습니다")
    fields = bots.validate(body, bot)
    if fields["capital"] != bot["capital"] and bot["state"]["paper"]["position"]["qty"] == 0:
        bot["state"]["paper"]["cash"] += fields["capital"] - bot["capital"]
    bot.update(fields)
    bots.log(bot, "config", "설정이 변경되었습니다")
    return jsonify(bot=bots.public(bots.save(bot)))


@bp.delete("/bots/<bot_id>")
@login_required
def delete_bot(bot_id: str):
    bot = bots.get(g.user["id"], bot_id)
    live = bot["state"]["live"]
    if live["position"]["qty"] > 0 or live["pending"]:
        raise ApiError(409, "bot_has_position", "실거래 보유분이나 미체결 주문이 있는 봇은 삭제할 수 없습니다")
    bots.delete(bot_id)
    return jsonify(ok=True)


@bp.post("/bots/<bot_id>/enable")
@login_required
def enable(bot_id: str):
    bot = bots.get(g.user["id"], bot_id)
    _need_toss()
    bot["enabled"] = True
    bots.log(bot, "config", f"켜짐 ({'모의' if bot['mode'] == 'paper' else '실거래'})")
    return jsonify(bot=bots.public(bots.save(bot)))


@bp.post("/bots/<bot_id>/disable")
@login_required
def disable(bot_id: str):
    bot = bots.get(g.user["id"], bot_id)
    bot["enabled"] = False
    bots.log(bot, "config", "꺼짐 (이미 들어간 주문·보유는 그대로입니다)")
    return jsonify(bot=bots.public(bots.save(bot)))


@bp.post("/bots/<bot_id>/mode")
@login_required
def set_mode(bot_id: str):
    """Switch paper <-> live. Going live needs a fresh OTP. Switching always turns the bot off."""
    bot = bots.get(g.user["id"], bot_id)
    body = request.get_json(silent=True) or {}
    mode = body.get("mode")
    if mode not in ("paper", "live"):
        raise ApiError(400, "invalid_mode")
    if mode == "live":
        _need_toss()
        otp.verify(_user(), str(body.get("otp_code", "")))
    bot["mode"] = mode
    bot["enabled"] = False
    bots.log(bot, "config", "실거래 모드로 전환 (꺼진 상태 — 켜면 실제 주문이 나갑니다)" if mode == "live" else "모의 모드로 전환", "warn" if mode == "live" else "info")
    return jsonify(bot=bots.public(bots.save(bot)))


@bp.post("/bots/<bot_id>/evaluate")
@login_required
def evaluate(bot_id: str):
    """What the bot would do on the next run (no orders, no state change)."""
    bot = bots.get(g.user["id"], bot_id)
    _need_toss()
    return jsonify(executor.preview(bot, target_date(bot["market"], datetime.now(timezone.utc))))


@bp.post("/bots/<bot_id>/run")
@login_required
def run_now(bot_id: str):
    """Paper only: run today's step now instead of waiting for the scheduler."""
    bot = bots.get(g.user["id"], bot_id)
    if bot["mode"] != "paper":
        raise ApiError(400, "paper_only", "즉시 실행은 모의 모드에서만 가능합니다 (실거래는 스케줄러가 실행)")
    _need_toss()
    sess = active_session(bot["market"], datetime.now(timezone.utc))
    if not sess:
        raise ApiError(409, "market_closed", "정규장 시간에만 실행할 수 있습니다")
    now = datetime.now(timezone.utc)
    result = executor.run_intraday(bot, sess[0], now, sess[2]) if executor.is_intraday(bot) else executor.run(bot, sess[0])
    bot = bots.get(g.user["id"], bot_id)
    return jsonify(result=result, bot=bots.public(bot))


@bp.post("/bots/<bot_id>/reset")
@login_required
def reset(bot_id: str):
    bot = bots.get(g.user["id"], bot_id)
    if bot["mode"] != "paper":
        raise ApiError(400, "paper_only", "모의 모드 기록만 초기화할 수 있습니다")
    bot["state"]["paper"] = bots.empty_state(bot["capital"])
    bots.log(bot, "config", "모의 기록 초기화")
    return jsonify(bot=bots.public(bots.save(bot)))
