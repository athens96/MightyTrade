import pyotp

from app.auth import routes as auth_routes

GOOGLE_INFO = {"sub": "g-123", "email": "me@example.com", "email_verified": True, "name": "Me"}


def _login(client, monkeypatch):
    monkeypatch.setattr(auth_routes, "verify_google_credential", lambda c: GOOGLE_INFO)
    return client.post("/api/auth/google", json={"credential": "x"})


def test_health(client):
    assert client.get("/api/health").json == {"status": "ok"}


def test_protected_requires_login(client):
    assert client.get("/api/market/candles?market=KR&symbol=005930").status_code == 401


def test_google_then_otp_setup_then_market(client, monkeypatch):
    r = _login(client, monkeypatch)
    assert r.json["next"] == "otp_setup"
    # pending session must not reach protected APIs
    assert client.get("/api/auth/me").status_code == 401

    secret = client.post("/api/auth/otp/setup").json["secret"]
    assert client.post("/api/auth/otp/verify", json={"code": "000000"}).status_code == 401
    r = client.post("/api/auth/otp/verify", json={"code": pyotp.TOTP(secret).now()})
    assert r.status_code == 200 and r.json["user"]["otp_enabled"]

    assert client.get("/api/auth/me").json["user"]["email"] == "me@example.com"
    r = client.get("/api/market/candles?market=US&symbol=AAPL&interval=1d&limit=50")
    assert r.status_code == 200 and len(r.json["candles"]) == 50

    # second login asks for verify, not setup
    client.post("/api/auth/logout")
    assert _login(client, monkeypatch).json["next"] == "otp_verify"


def test_otp_lockout(client, monkeypatch):
    _login(client, monkeypatch)
    client.post("/api/auth/otp/setup")
    codes = [client.post("/api/auth/otp/verify", json={"code": "000000"}).status_code for _ in range(6)]
    assert codes[-1] == 429


def test_unimplemented_order_returns_501(client, monkeypatch):
    _login(client, monkeypatch)
    secret = client.post("/api/auth/otp/setup").json["secret"]
    client.post("/api/auth/otp/verify", json={"code": pyotp.TOTP(secret).now()})
    assert client.post("/api/trading/orders", json={}).status_code == 501
