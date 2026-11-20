"""Shared test doubles."""
import pyotp
import pytest

from app.auth import routes as auth_routes


class FakeToss:
    def __init__(self, routes):
        self.routes = routes
        self.calls = []

    def request(self, method, path, *, params=None, json=None, account_seq=None):
        self.calls.append((method, path, params, json, account_seq))
        r = self.routes.get((method, path))
        if isinstance(r, Exception):
            raise r
        return r(params, json) if callable(r) else r



def approve_all(client):
    """Tests act as an admin who approved every sign-up (accounts start pending)."""
    with client.application.app_context():
        from app.storage import get_store
        store = get_store()
        for u in store.find("users"):
            store.put("users", u["id"], {**u, "status": "approved"})


def login(client, monkeypatch, sub="a"):
    """Google login + OTP enrollment. Returns the TOTP (the current code is already used)."""
    monkeypatch.setattr(auth_routes, "verify_google_credential",
                        lambda c: {"sub": sub, "email": f"{sub}@example.com", "email_verified": True})
    client.post("/api/auth/google", json={"credential": "x"})
    approve_all(client)
    client.post("/api/auth/google", json={"credential": "x"})
    totp = pyotp.TOTP(client.post("/api/auth/otp/setup").json["secret"])
    client.post("/api/auth/otp/verify", json={"code": totp.now()})
    return totp
