"""User requests submission and strict admin-only inbox tests (MT-16)."""
import pyotp
import pytest

from app.auth import routes as auth_routes
from app.autotrade import notifier
from app.storage import get_store
from app.users import credentials
from fakes import login


def _login_admin(app, monkeypatch):
    c = app.test_client()
    monkeypatch.setattr(
        auth_routes,
        "verify_google_credential",
        lambda _: {"sub": "adm-req", "email": "admin@example.com", "email_verified": True, "name": "운영자"},
    )
    r = c.post("/api/auth/google", json={"credential": "x"})
    assert r.json["next"] == "otp_setup" and r.json["user"]["role"] == "admin"
    totp = pyotp.TOTP(c.post("/api/auth/otp/setup").json["secret"])
    c.post("/api/auth/otp/verify", json={"code": totp.now()})
    return c


def test_unauthenticated_access_blocked(client):
    assert client.get("/api/users/me/requests").status_code == 401
    assert client.post("/api/users/me/requests", json={"category": "feature", "title": "제목", "content": "내용입니다123"}).status_code == 401
    assert client.get("/api/admin/requests").status_code == 401


def test_validation_and_rate_limit(app, client, monkeypatch):
    login(client, monkeypatch, sub="u_val")

    assert client.post("/api/users/me/requests", json={"category": "invalid", "title": "제목", "content": "충분히 긴 요청 내용"}).json["error"] == "invalid_category"
    assert client.post("/api/users/me/requests", json={"category": "feature", "title": "짧", "content": "충분히 긴 요청 내용"}).json["error"] == "invalid_title"
    assert client.post("/api/users/me/requests", json={"category": "feature", "title": "정상 제목", "content": "짧음"}).json["error"] == "invalid_content"

    for idx in range(10):
        res = client.post(
            "/api/users/me/requests",
            json={"category": "feature", "title": f"건의 {idx}", "content": "새로운 지표를 추가해 주세요!"},
        )
        assert res.status_code == 201
    over = client.post(
        "/api/users/me/requests",
        json={"category": "feature", "title": "11번째 건의", "content": "추가 요청을 보내봅니다!"},
    )
    assert over.status_code == 429 and over.json["error"] == "too_many_open_requests"


def test_only_admins_receive_and_manage_user_requests(app, monkeypatch):
    user_a = app.test_client()
    user_b = app.test_client()
    login(user_a, monkeypatch, sub="alice")
    login(user_b, monkeypatch, sub="bob")
    admin = _login_admin(app, monkeypatch)

    # Configure notification webhooks for BOTH admin and non-admin user_b.
    # Only admin's webhook should ever receive the incoming user request alert!
    sent_calls: list[tuple[str, dict]] = []
    monkeypatch.setattr(notifier, "_send_http", lambda url, payload: sent_calls.append((url, payload)))

    with app.app_context():
        store = get_store()
        admin_uid = store.find_one("users", lambda u: u.get("email") == "admin@example.com")["id"]
        bob_uid = store.find_one("users", lambda u: u.get("email") == "bob@example.com")["id"]
        credentials.save_notifications(admin_uid, {"enabled": True, "slack_webhook_url": "https://hooks.slack.com/services/ADMIN/HOOK"})
        credentials.save_notifications(bob_uid, {"enabled": True, "slack_webhook_url": "https://hooks.slack.com/services/BOB/HOOK"})

    # Alice submits a feature request
    created = user_a.post(
        "/api/users/me/requests",
        json={
            "category": "strategy",
            "title": "일목균형표 전략 추가 건의",
            "content": "퀀트 랩과 자동매매 스윙봇에서도 일목균형표 구름대 돌파 전략을 선택할 수 있으면 좋겠습니다.",
            "page": "/quant",
            "app_version": "0.1.4",
        },
    )
    assert created.status_code == 201
    req_id = created.json["item"]["id"]
    assert "replied_by" not in created.json["item"]

    # Real-time alert delivered ONLY to admin, never to Bob
    assert len(sent_calls) == 1
    assert sent_calls[0][0] == "https://hooks.slack.com/services/ADMIN/HOOK"
    assert "일목균형표 전략 추가 건의" in sent_calls[0][1]["text"]
    assert "alice@example.com" in sent_calls[0][1]["text"]

    # Strict isolation: Bob cannot see Alice's request in /me/requests, and gets 403 on every /admin/requests endpoint
    assert user_b.get("/api/users/me/requests").json == {"items": []}
    assert user_b.get("/api/admin/summary").status_code == 403
    assert user_b.get("/api/admin/requests").status_code == 403
    assert user_b.patch(f"/api/admin/requests/{req_id}", json={"status": "resolved"}).status_code == 403
    assert user_b.delete(f"/api/admin/requests/{req_id}").status_code == 403

    # Alice also gets 403 on admin endpoints, even for her own request ID
    assert user_a.get("/api/admin/requests").status_code == 403
    assert user_a.patch(f"/api/admin/requests/{req_id}", json={"status": "resolved"}).status_code == 403
    assert user_a.delete(f"/api/admin/requests/{req_id}").status_code == 403

    # Admin sees badge count and full inbox entry
    assert admin.get("/api/admin/summary").json == {"pending": 0, "open_requests": 1}
    inbox = admin.get("/api/admin/requests?status=open").json
    assert inbox["counts"] == {"open": 1, "in_review": 0, "resolved": 0, "closed": 0, "all": 1}
    assert len(inbox["items"]) == 1
    assert inbox["items"][0]["user_email"] == "alice@example.com"
    assert inbox["items"][0]["page"] == "/quant"

    # Admin replies and marks resolved
    patched = admin.patch(
        f"/api/admin/requests/{req_id}",
        json={"status": "resolved", "admin_reply": "좋은 의견 감사합니다! 다음 업데이트에 추가하겠습니다."},
    )
    assert patched.status_code == 200
    assert patched.json["item"]["status"] == "resolved"
    assert patched.json["item"]["replied_by"] == "admin@example.com"

    # Open badge drops to 0; Alice sees the reply on her own list without leaking admin email
    assert admin.get("/api/admin/summary").json == {"pending": 0, "open_requests": 0}
    alice_items = user_a.get("/api/users/me/requests").json["items"]
    assert len(alice_items) == 1
    assert alice_items[0]["status"] == "resolved"
    assert alice_items[0]["admin_reply"] == "좋은 의견 감사합니다! 다음 업데이트에 추가하겠습니다."
    assert "replied_by" not in alice_items[0]

    # Admin can delete the request
    assert admin.delete(f"/api/admin/requests/{req_id}").status_code == 200
    assert admin.delete(f"/api/admin/requests/{req_id}").status_code == 404
    assert user_a.get("/api/users/me/requests").json == {"items": []}
