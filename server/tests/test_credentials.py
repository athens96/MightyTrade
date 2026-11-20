"""Per-account Toss key registration: transport encryption, OTP step-up, encryption at rest."""
import base64
import json
import os
import time

import pyotp
import pytest
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from fakes import approve_all
from app.auth import routes as auth_routes
from app.errors import ApiError
from app.integrations.toss import client as toss_client

SECRET = "tssk_live_super_secret_value_1234567890"
CLIENT_ID = "tsck_live_abcdefghijklmnop"
GOOGLE_INFO = {"sub": "g-1", "email": "me@example.com", "email_verified": True, "name": "Me"}


def seal(key_info, payload, aad):
    """What the browser does with WebCrypto (client/src/lib/secureEnvelope.ts)."""
    pub = serialization.load_der_public_key(base64.b64decode(key_info["spki"]))
    aes = os.urandom(32)
    iv = os.urandom(12)
    ct = AESGCM(aes).encrypt(iv, json.dumps(payload).encode(), aad.encode())
    ek = pub.encrypt(aes, padding.OAEP(mgf=padding.MGF1(hashes.SHA256()), algorithm=hashes.SHA256(), label=None))
    b64 = lambda b: base64.b64encode(b).decode()  # noqa: E731
    return {"kid": key_info["kid"], "alg": key_info["alg"], "ek": b64(ek), "iv": b64(iv), "ct": b64(ct)}


@pytest.fixture
def session(client, monkeypatch):
    """Logged-in client. Returns (client, user_id, totp)."""
    monkeypatch.setattr(auth_routes, "verify_google_credential", lambda c: GOOGLE_INFO)
    client.post("/api/auth/google", json={"credential": "x"})
    approve_all(client)
    uid = client.post("/api/auth/google", json={"credential": "x"}).json["user"]["id"]
    totp = pyotp.TOTP(client.post("/api/auth/otp/setup").json["secret"])
    assert client.post("/api/auth/otp/verify", json={"code": totp.now()}).status_code == 200
    return client, uid, totp


@pytest.fixture
def fake_verify(monkeypatch):
    calls = []

    def _verify(cid, secret):
        calls.append((cid, secret))
        return 7
    monkeypatch.setattr("app.users.routes.verify_credentials", _verify)
    return calls


def next_code(totp):
    # The current code was already used for login (single-use); the next step is within the drift window.
    return totp.at(time.time() + 30)


def register(c, uid, otp_code, **overrides):
    key = c.get("/api/users/transport-key").json
    payload = {"client_id": CLIENT_ID, "client_secret": SECRET, "otp_code": otp_code, "iat": int(time.time())}
    payload.update(overrides.pop("payload", {}))
    env = seal(key, payload, overrides.pop("aad", uid))
    return c.put("/api/users/me/broker/toss", json={"envelope": env})


def test_register_encrypts_at_rest_and_never_returns_secret(app, session, fake_verify, tmp_path):
    c, uid, totp = session
    r = register(c, uid, next_code(totp))
    assert r.status_code == 200, r.json
    assert r.json == {"configured": True, "client_id_masked": "tsck_live_…mnop", "account_seq": 7,
                      "verified_at": r.json["verified_at"]}
    assert fake_verify == [(CLIENT_ID, SECRET)]
    assert SECRET not in json.dumps(c.get("/api/users/me/broker/toss").json)
    # nothing in the data dir contains the plaintext secret or client id
    for f in tmp_path.glob("*.json"):
        text = f.read_text()
        assert SECRET not in text and CLIENT_ID not in text, f.name
    with app.app_context():
        from app.users import credentials
        assert credentials.get_toss(uid)["client_secret"] == SECRET


def test_register_requires_fresh_otp(session, fake_verify):
    c, uid, totp = session
    assert register(c, uid, "000000").json["error"] == "invalid_otp"
    assert register(c, uid, totp.now()).json["error"] == "invalid_otp"  # already used for login
    assert fake_verify == []
    assert c.get("/api/users/me/broker/toss").json == {"configured": False}


def test_envelope_bound_to_user(session, fake_verify):
    c, uid, totp = session
    r = register(c, uid, next_code(totp), aad="someone-else")
    assert r.status_code == 400 and r.json["error"] == "invalid_envelope"


def test_envelope_expires(session, fake_verify):
    c, uid, totp = session
    r = register(c, uid, next_code(totp), payload={"iat": int(time.time()) - 3600})
    assert r.json["error"] == "envelope_expired"


def test_plaintext_body_rejected(session, fake_verify):
    c, _, _ = session
    r = c.put("/api/users/me/broker/toss", json={"client_id": CLIENT_ID, "client_secret": SECRET})
    assert r.status_code == 400 and r.json["error"] == "invalid_envelope"


def test_delete(session, fake_verify):
    c, uid, totp = session
    register(c, uid, next_code(totp))
    assert c.delete("/api/users/me/broker/toss").json == {"configured": False}


def test_toss_market_without_key_asks_to_register(app, session):
    c, _, _ = session
    app.config["MARKET_PROVIDERS"] = {"KR": "toss", "US": "toss"}
    r = c.get("/api/market/candles?market=KR&symbol=005930")
    assert r.status_code == 409 and r.json["error"] == "toss_credentials_required"


def test_verify_maps_bad_key_to_400(app, monkeypatch):
    def bad_token(self):
        raise ApiError(502, "toss_auth_invalid_client", "bad")
    monkeypatch.setattr(toss_client.TossClient, "_issue_token", bad_token)
    with app.app_context():
        with pytest.raises(ApiError) as e:
            toss_client.verify_credentials("id", "secret")
    assert e.value.status == 400 and "올바르지" in e.value.message
