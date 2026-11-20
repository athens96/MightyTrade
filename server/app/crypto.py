"""Encryption at rest for secrets stored in the DB (OTP secrets, broker API keys)."""
from __future__ import annotations

import base64
import hashlib

from cryptography.fernet import Fernet
from flask import current_app


def _fernet() -> Fernet:
    key = current_app.config["ENCRYPTION_KEY"]
    if not key:
        # Dev fallback: derive from SECRET_KEY. Production requires ENCRYPTION_KEY (checked in create_app).
        digest = hashlib.sha256(current_app.config["SECRET_KEY"].encode()).digest()
        key = base64.urlsafe_b64encode(digest).decode()
    return Fernet(key)


def encrypt(plain: str) -> str:
    return _fernet().encrypt(plain.encode()).decode()


def decrypt(token: str) -> str:
    return _fernet().decrypt(token.encode()).decode()
