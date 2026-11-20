"""Admin: sign-up approval and account deletion. Admins are the emails in ADMIN_EMAILS."""
from __future__ import annotations

from flask import Blueprint, g, jsonify, request

from ..auth import otp
from ..auth.decorators import admin_required
from ..autotrade import bots
from ..errors import ApiError
from ..integrations.toss import forget_client
from ..storage import get_store
from ..users import credentials
from ..users import repository as users
from ..users.repository import now_iso

bp = Blueprint("admin", __name__)

# Per-account data removed on deletion (collection -> how records point at the user)
BY_ID = ("users", "watchlists", "settings", "broker_credentials")
BY_USER_ID = ("bots", "bot_events", "orders")


def _summary(u: dict) -> dict:
    user_bots = [b for b in get_store().find("bots", lambda b: b.get("user_id") == u["id"])]
    return {
        **users.public_view(u),
        "created_at": u.get("created_at"),
        "last_login_at": u.get("last_login_at"),
        "approved_at": u.get("approved_at"),
        "approved_by": u.get("approved_by"),
        "has_toss_key": bool((get_store().get("broker_credentials", u["id"]) or {}).get("toss")),
        "bots": len(user_bots),
        "live_bots": sum(1 for b in user_bots if b.get("mode") == "live" and b.get("enabled")),
    }


def _target(user_id: str) -> dict:
    u = users.get_user(user_id)
    if u is None:
        raise ApiError(404, "user_not_found")
    return u


def _guard(u: dict, action: str) -> None:
    if u["id"] == g.user["id"]:
        raise ApiError(400, "cannot_target_self", f"자기 자신은 {action}할 수 없습니다")
    if users.is_admin(u):
        raise ApiError(400, "cannot_target_admin", f"관리자 계정은 {action}할 수 없습니다 (ADMIN_EMAILS에서 먼저 빼세요)")


@bp.get("/summary")
@admin_required
def summary():
    """Badge counts for the admin menu."""
    all_users = get_store().find("users")
    return jsonify(pending=sum(1 for u in all_users if users.status_of(u) == "pending"))


@bp.get("/users")
@admin_required
def list_users():
    status = request.args.get("status", "all")
    if status not in ("all", *users.STATUSES):
        raise ApiError(400, "invalid_status")
    all_users = get_store().find("users")
    counts = {s: sum(1 for u in all_users if users.status_of(u) == s) for s in users.STATUSES}
    rows = [u for u in all_users if status == "all" or users.status_of(u) == status]
    rows.sort(key=lambda u: (users.status_of(u) != "pending", u.get("created_at") or ""), reverse=False)
    return jsonify(items=[_summary(u) for u in rows], counts={**counts, "all": len(all_users)})


@bp.post("/users/<user_id>/approve")
@admin_required
def approve(user_id: str):
    u = _target(user_id)
    u = users.update_user(u["id"], status="approved", approved_at=now_iso(), approved_by=g.user.get("email"))
    return jsonify(user=_summary(u))


@bp.post("/users/<user_id>/reject")
@admin_required
def reject(user_id: str):
    u = _target(user_id)
    _guard(u, "거절")
    u = users.update_user(u["id"], status="rejected", approved_at=None, approved_by=g.user.get("email"))
    for b in bots.list_for(u["id"]):  # a rejected account must not keep trading
        if b.get("enabled"):
            b["enabled"] = False
            bots.log(b, "config", "계정 승인이 취소되어 꺼졌습니다", "warn")
            bots.save(b)
    return jsonify(user=_summary(u))


@bp.delete("/users/<user_id>")
@admin_required
def delete_user(user_id: str):
    """Delete an account and all its data. Needs the admin's fresh OTP code."""
    body = request.get_json(silent=True) or {}
    u = _target(user_id)
    _guard(u, "삭제")
    otp.verify(users.get_user(g.user["id"]) or g.user, str(body.get("otp_code", "")))

    user_bots = bots.list_for(u["id"])
    live_open = [b for b in user_bots if b["state"]["live"]["position"]["qty"] > 0 or b["state"]["live"]["pending"]]
    if live_open and not body.get("force"):
        raise ApiError(409, "live_positions", f"실거래 봇 {len(live_open)}개에 보유·미체결 주문이 있습니다. "
                                              "사용자가 정리한 뒤 삭제하세요 (토스 계좌의 주문·보유는 삭제와 무관하게 남습니다)")

    store = get_store()
    creds = credentials.get_toss(u["id"])
    if creds:
        forget_client(u["id"], creds["client_id"])  # drop the cached Toss token too
    removed: dict = {}
    for coll in BY_USER_ID:
        rows = store.find(coll, lambda r: r.get("user_id") == u["id"])
        for r in rows:
            store.delete(coll, r["id"])
        removed[coll] = len(rows)
    for coll in BY_ID:
        removed[coll] = int(store.delete(coll, u["id"]))
    return jsonify(ok=True, removed=removed)
