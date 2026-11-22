"""User feedback / feature requests repository (`user_requests` collection, MT-16).

Security invariant:
- Any approved, logged-in user can submit a request and view ONLY their own submitted items (`list_for_user`).
- ONLY administrators (`ADMIN_EMAILS`, enforced via `@admin_required` and `users.is_admin`) receive real-time
  notifications, view all incoming requests, update statuses/replies, or delete requests.
"""
from __future__ import annotations

import logging
import uuid
from typing import Any

from ..autotrade import notifier
from ..errors import ApiError
from ..storage import get_store
from . import credentials
from . import repository as users
from .repository import now_iso

log = logging.getLogger(__name__)

USER_REQUESTS = "user_requests"

CATEGORY_LABELS: dict[str, str] = {
    "feature": "기능 건의",
    "bug": "오류·버그 제보",
    "strategy": "전략·지표 추가",
    "other": "기타 문의",
}
CATEGORIES = tuple(CATEGORY_LABELS.keys())

STATUS_LABELS: dict[str, str] = {
    "open": "접수 대기",
    "in_review": "검토 중",
    "resolved": "반영 완료",
    "closed": "보류·종료",
}
STATUSES = tuple(STATUS_LABELS.keys())
STATUS_ORDER = {"open": 0, "in_review": 1, "resolved": 2, "closed": 3}

MAX_OPEN_PER_USER = 10


def user_view(rec: dict[str, Any]) -> dict[str, Any]:
    """Safe view for the author of the request (never exposes admin emails or other users)."""
    return {
        "id": rec["id"],
        "category": rec.get("category", "other"),
        "title": rec.get("title", ""),
        "content": rec.get("content", ""),
        "page": rec.get("page"),
        "app_version": rec.get("app_version"),
        "status": rec.get("status", "open"),
        "admin_reply": rec.get("admin_reply") or "",
        "replied_at": rec.get("replied_at"),
        "created_at": rec.get("created_at"),
        "updated_at": rec.get("updated_at"),
    }


def admin_view(rec: dict[str, Any]) -> dict[str, Any]:
    """Full view restricted strictly to administrators."""
    return {
        **user_view(rec),
        "user_id": rec.get("user_id"),
        "user_email": rec.get("user_email"),
        "user_name": rec.get("user_name"),
        "replied_by": rec.get("replied_by"),
    }


def notify_admins_new_request(rec: dict[str, Any]) -> list[str]:
    """Deliver a real-time alert ONLY to admin accounts that configured notification channels."""
    delivered_admins: list[str] = []
    try:
        all_users = get_store().find("users")
        admins = [u for u in all_users if users.is_admin(u)]
        if not admins:
            return delivered_admins

        cat_label = CATEGORY_LABELS.get(rec.get("category", ""), rec.get("category", "기타"))
        sender = f"{rec.get('user_name') or '사용자'} ({rec.get('user_email') or '-'})"
        meta_parts = [cat_label]
        if rec.get("page"):
            meta_parts.append(f"화면 {rec['page']}")
        if rec.get("app_version"):
            meta_parts.append(f"v{rec['app_version']}")
        body_preview = rec["content"] if len(rec["content"]) <= 350 else f"{rec['content'][:350]}…"
        text = (
            "📩 [Mighty Trade · 사용자 요청사항 접수]\n"
            f"보낸 사람: {sender}\n"
            f"분류: {' · '.join(meta_parts)}\n"
            f"제목: {rec['title']}\n"
            f"내용: {body_preview}"
        )
        for admin_user in admins:
            cfg = credentials.get_notifications(admin_user["id"])
            if not cfg or not cfg.get("enabled", True):
                continue
            sent, errors = notifier._deliver(cfg, text)
            if sent:
                delivered_admins.append(admin_user["id"])
            if errors:
                log.warning("admin request notification errors for %s: %s", admin_user["id"][:8], errors)
    except Exception:  # noqa: BLE001
        log.debug("notify_admins_new_request suppressed error", exc_info=True)
    return delivered_admins


def create_request(user: dict[str, Any], body: dict[str, Any]) -> dict[str, Any]:
    category = str(body.get("category") or "feature").strip()
    if category not in CATEGORIES:
        raise ApiError(400, "invalid_category", "요청 분류를 올바르게 선택해 주세요")

    title = str(body.get("title") or "").strip()
    if not (2 <= len(title) <= 100):
        raise ApiError(400, "invalid_title", "제목은 2자 이상 100자 이하로 입력해 주세요")

    content = str(body.get("content") or "").strip()
    if not (5 <= len(content) <= 2000):
        raise ApiError(400, "invalid_content", "요청 내용은 5자 이상 2,000자 이하로 적어 주세요")

    page = str(body.get("page") or "").strip()[:120] or None
    app_version = str(body.get("app_version") or "").strip()[:32] or None

    store = get_store()
    open_cnt = sum(
        1
        for r in store.find(
            USER_REQUESTS,
            lambda r: r.get("user_id") == user["id"] and r.get("status") == "open",
        )
    )
    if open_cnt >= MAX_OPEN_PER_USER:
        raise ApiError(
            429,
            "too_many_open_requests",
            f"아직 접수 대기 중인 요청이 {MAX_OPEN_PER_USER}건 있습니다. 관리자 검토 후 추가로 보내 주세요.",
        )

    req_id = uuid.uuid4().hex[:12]
    ts = now_iso()
    rec = {
        "id": req_id,
        "user_id": user["id"],
        "user_email": user.get("email"),
        "user_name": user.get("name"),
        "category": category,
        "title": title,
        "content": content,
        "page": page,
        "app_version": app_version,
        "status": "open",
        "admin_reply": "",
        "replied_at": None,
        "replied_by": None,
        "created_at": ts,
        "updated_at": ts,
    }
    saved = store.put(USER_REQUESTS, req_id, rec)
    notify_admins_new_request(saved)
    return user_view(saved)


def list_for_user(user_id: str) -> list[dict[str, Any]]:
    rows = get_store().find(USER_REQUESTS, lambda r: r.get("user_id") == user_id)
    rows.sort(key=lambda r: r.get("created_at") or "", reverse=True)
    return [user_view(r) for r in rows]


def open_count() -> int:
    return sum(1 for r in get_store().find(USER_REQUESTS) if r.get("status") == "open")


def list_all_for_admin(status: str = "all") -> tuple[list[dict[str, Any]], dict[str, int]]:
    if status not in ("all", *STATUSES):
        raise ApiError(400, "invalid_status")
    all_rows = get_store().find(USER_REQUESTS)
    counts = {s: sum(1 for r in all_rows if r.get("status") == s) for s in STATUSES}
    rows = [r for r in all_rows if status == "all" or r.get("status") == status]
    rows.sort(key=lambda r: r.get("created_at") or "", reverse=True)
    rows.sort(key=lambda r: STATUS_ORDER.get(r.get("status", "open"), 9))
    return [admin_view(r) for r in rows], {**counts, "all": len(all_rows)}


def update_by_admin(req_id: str, admin_user: dict[str, Any], body: dict[str, Any]) -> dict[str, Any]:
    store = get_store()
    rec = store.get(USER_REQUESTS, req_id)
    if rec is None:
        raise ApiError(404, "request_not_found", "해당 요청사항을 찾을 수 없습니다")

    if "status" in body and body["status"] is not None:
        new_status = str(body["status"]).strip()
        if new_status not in STATUSES:
            raise ApiError(400, "invalid_status")
        rec["status"] = new_status

    if "admin_reply" in body and body["admin_reply"] is not None:
        reply = str(body["admin_reply"]).strip()
        if len(reply) > 2000:
            raise ApiError(400, "reply_too_long", "관리자 답변은 2,000자 이하로 입력해 주세요")
        rec["admin_reply"] = reply
        rec["replied_at"] = now_iso() if reply else None
        rec["replied_by"] = admin_user.get("email") if reply else None

    rec["updated_at"] = now_iso()
    saved = store.put(USER_REQUESTS, req_id, rec)
    return admin_view(saved)


def delete_by_admin(req_id: str) -> None:
    if not get_store().delete(USER_REQUESTS, req_id):
        raise ApiError(404, "request_not_found", "해당 요청사항을 찾을 수 없습니다")
