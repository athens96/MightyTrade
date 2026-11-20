"""Sign-up approval and the admin API."""
import time

import pyotp
import pytest

from app.auth import routes as auth_routes
from app.autotrade import bots
from app.storage import get_store


def google(client, monkeypatch, sub, email):
    monkeypatch.setattr(auth_routes, "verify_google_credential",
                        lambda c: {"sub": sub, "email": email, "email_verified": True, "name": sub})
    return client.post("/api/auth/google", json={"credential": "x"})


@pytest.fixture
def admin(app, monkeypatch):
    """Logged-in admin client (separate cookie jar from the users being managed)."""
    c = app.test_client()
    r = google(c, monkeypatch, "adm", "Admin@Example.com")  # case-insensitive match
    assert r.json["next"] == "otp_setup" and r.json["user"]["role"] == "admin"
    totp = pyotp.TOTP(c.post("/api/auth/otp/setup").json["secret"])
    c.post("/api/auth/otp/verify", json={"code": totp.now()})
    return c, totp


def test_new_signup_waits_for_approval(app, client, monkeypatch):
    r = google(client, monkeypatch, "u1", "user@example.com")
    assert r.json["next"] == "approval_pending" and r.json["user"]["status"] == "pending"
    pending = client.get_cookie("mt_pending", path="/api/auth")
    assert pending is None or pending.value == ""  # no OTP step yet (the header only clears it)
    assert client.post("/api/auth/otp/setup").status_code == 401
    assert client.get("/api/auth/me").status_code == 401


def test_approve_then_normal_login(app, client, admin, monkeypatch):
    a, _ = admin
    uid = google(client, monkeypatch, "u1", "user@example.com").json["user"]["id"]
    assert a.get("/api/admin/summary").json == {"pending": 1}
    listing = a.get("/api/admin/users?status=pending").json
    assert [u["email"] for u in listing["items"]] == ["user@example.com"] and listing["counts"]["pending"] == 1
    r = a.post(f"/api/admin/users/{uid}/approve")
    assert r.json["user"]["status"] == "approved" and r.json["user"]["approved_by"] == "Admin@Example.com"
    assert google(client, monkeypatch, "u1", "user@example.com").json["next"] == "otp_setup"


def test_reject_blocks_and_stops_bots(app, client, admin, monkeypatch):
    a, _ = admin
    uid = google(client, monkeypatch, "u1", "user@example.com").json["user"]["id"]
    a.post(f"/api/admin/users/{uid}/approve")
    with app.app_context():
        b = bots.create(uid, {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "capital": 1e6})
        b["enabled"] = True
        bots.save(b)
    a.post(f"/api/admin/users/{uid}/reject")
    assert google(client, monkeypatch, "u1", "user@example.com").json["next"] == "rejected"
    with app.app_context():
        assert bots.list_for(uid)[0]["enabled"] is False


def test_non_admin_cannot_use_admin_api(app, client, monkeypatch):
    from fakes import login
    login(client, monkeypatch, sub="plain")
    assert client.get("/api/admin/users").status_code == 403


def test_delete_requires_otp_and_purges_data(app, client, admin, monkeypatch):
    a, totp = admin
    uid = google(client, monkeypatch, "u1", "user@example.com").json["user"]["id"]
    with app.app_context():
        store = get_store()
        store.put("watchlists", uid, {"items": []})
        store.put("settings", uid, {"values": {}})
        store.put("orders", "o1", {"user_id": uid, "date": "x"})
        bots.create(uid, {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "capital": 1e6})
    assert a.delete(f"/api/admin/users/{uid}", json={"otp_code": "000000"}).status_code == 401
    r = a.delete(f"/api/admin/users/{uid}", json={"otp_code": totp.at(time.time() + 30)})
    assert r.status_code == 200 and r.json["removed"]["users"] == 1 and r.json["removed"]["bots"] == 1
    with app.app_context():
        store = get_store()
        assert store.get("users", uid) is None and store.get("watchlists", uid) is None
        assert not store.find("orders", lambda o: o.get("user_id") == uid)
    # the deleted person can sign up again, as a new pending account
    assert google(client, monkeypatch, "u1", "user@example.com").json["next"] == "approval_pending"


def test_delete_guards(app, client, admin, monkeypatch):
    a, totp = admin
    with app.app_context():
        me = get_store().find_one("users", lambda u: u["google_sub"] == "adm")
    r = a.delete(f"/api/admin/users/{me['id']}", json={"otp_code": totp.at(time.time() + 30)})
    assert r.json["error"] == "cannot_target_self"
    uid = google(client, monkeypatch, "u1", "user@example.com").json["user"]["id"]
    with app.app_context():
        b = bots.create(uid, {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "capital": 1e6})
        b["state"]["live"]["position"]["qty"] = 5
        bots.save(b)
    # the self-delete above was refused before the OTP check, so the code is still unused
    r = a.delete(f"/api/admin/users/{uid}", json={"otp_code": totp.at(time.time() + 30)})
    assert r.status_code == 409 and r.json["error"] == "live_positions"


def test_legacy_accounts_are_grandfathered(app, client, monkeypatch):
    with app.app_context():
        get_store().put("users", "old", {"google_sub": "legacy", "email": "old@example.com", "otp_enabled": False})
    assert google(client, monkeypatch, "legacy", "old@example.com").json["next"] == "otp_setup"
