"""Application-layer encryption for secrets sent from the browser (e.g. broker API keys).

HTTPS already encrypts the connection; this adds a second layer so secrets never appear in
plaintext in request bodies (devtools, proxy/WAF logs, crash reports, accidental logging).

Envelope (hybrid encryption, all values base64):
    {"kid": "...", "alg": "RSA-OAEP-256+A256GCM", "ek": wrapped AES key, "iv": 12-byte nonce, "ct": ciphertext+tag}

- A fresh AES-256-GCM key per message encrypts the JSON payload.
- That key is encrypted with the server's RSA-3072 public key (OAEP, SHA-256).
- AES-GCM additional data = the logged-in user's id, so an envelope can't be replayed into another account.
- Payload must contain "iat" (unix seconds); stale envelopes are rejected.

The RSA private key is generated once, stored encrypted (ENCRYPTION_KEY) in the Store and shared by
all worker processes. It never leaves the server.
"""
from __future__ import annotations

import base64
import hashlib
import json
import time
from typing import Any

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from flask import current_app

from .crypto import decrypt, encrypt
from .errors import ApiError
from .storage import get_store

ALG = "RSA-OAEP-256+A256GCM"
KEYS = "server_keys"
KEY_ID = "transport_rsa"
MAX_AGE_SEC = 300
OAEP = padding.OAEP(mgf=padding.MGF1(algorithm=hashes.SHA256()), algorithm=hashes.SHA256(), label=None)


def _b64d(value: Any) -> bytes:
    if not isinstance(value, str):
        raise ValueError("expected base64 string")
    return base64.b64decode(value, validate=True)


def _load_or_create() -> tuple[rsa.RSAPrivateKey, str, str]:
    """Return (private_key, kid, spki_b64), cached per process."""
    cached = current_app.extensions.get("transport_key")
    if cached:
        return cached

    store = get_store()
    with store.lock(KEY_ID):
        rec = store.get(KEYS, KEY_ID)
        if rec:
            pem = decrypt(rec["private_pem_enc"]).encode()
            key = serialization.load_pem_private_key(pem, password=None)
        else:
            key = rsa.generate_private_key(public_exponent=65537, key_size=3072)
            pem = key.private_bytes(
                serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()
            )
            store.put(KEYS, KEY_ID, {"private_pem_enc": encrypt(pem.decode()), "created_at": int(time.time())})

    spki = key.public_key().public_bytes(serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo)
    kid = hashlib.sha256(spki).hexdigest()[:16]
    result = (key, kid, base64.b64encode(spki).decode())
    current_app.extensions["transport_key"] = result
    return result


def public_key_info() -> dict[str, str]:
    _, kid, spki = _load_or_create()
    return {"kid": kid, "alg": ALG, "spki": spki}


def open_envelope(envelope: Any, aad: str) -> dict[str, Any]:
    """Decrypt and validate an envelope from the client. Raises ApiError(400) on any failure."""
    if not isinstance(envelope, dict) or envelope.get("alg") != ALG:
        raise ApiError(400, "invalid_envelope", "unsupported or missing envelope")
    key, kid, _ = _load_or_create()
    if envelope.get("kid") != kid:
        # Server key changed (e.g. data dir reset): client must refetch the public key.
        raise ApiError(400, "stale_transport_key", "public key changed, please retry")
    try:
        aes_key = key.decrypt(_b64d(envelope.get("ek")), OAEP)
        plain = AESGCM(aes_key).decrypt(_b64d(envelope.get("iv")), _b64d(envelope.get("ct")), aad.encode())
        payload = json.loads(plain)
    except (ValueError, InvalidTag, TypeError):
        raise ApiError(400, "invalid_envelope", "could not decrypt payload")
    if not isinstance(payload, dict):
        raise ApiError(400, "invalid_envelope", "payload must be an object")

    iat = payload.get("iat")
    if not isinstance(iat, (int, float)) or abs(time.time() - iat) > MAX_AGE_SEC:
        raise ApiError(400, "envelope_expired", "payload is too old or clock is off")
    return payload
