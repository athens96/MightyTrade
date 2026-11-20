"""Per-account broker API credentials, encrypted at rest.

Record (collection `broker_credentials`, id = user id):
    toss: {client_id_enc, client_secret_enc, client_id_masked, account_seq, verified_at, updated_at}

Secrets are only ever decrypted server-side to call the broker. Nothing here is returned to the
client except `public_view()` (masked id + status).
"""
from __future__ import annotations

from typing import Any, Optional

from ..crypto import decrypt, encrypt
from ..storage import get_store
from .repository import now_iso

CREDENTIALS = "broker_credentials"


def mask(value: str) -> str:
    if len(value) <= 12:
        return value[:2] + "…"
    return f"{value[:10]}…{value[-4:]}"


def save_toss(user_id: str, client_id: str, client_secret: str, account_seq: Optional[int]) -> dict[str, Any]:
    store = get_store()
    rec = store.get(CREDENTIALS, user_id) or {}
    rec["toss"] = {
        "client_id_enc": encrypt(client_id),
        "client_secret_enc": encrypt(client_secret),
        "client_id_masked": mask(client_id),
        "account_seq": account_seq,
        "verified_at": now_iso(),
        "updated_at": now_iso(),
    }
    return store.put(CREDENTIALS, user_id, rec)["toss"]


def get_toss(user_id: str) -> Optional[dict[str, Any]]:
    """Decrypted credentials for server-side use only. Never serialize this to a response."""
    rec = (get_store().get(CREDENTIALS, user_id) or {}).get("toss")
    if not rec:
        return None
    return {
        "client_id": decrypt(rec["client_id_enc"]),
        "client_secret": decrypt(rec["client_secret_enc"]),
        "account_seq": rec.get("account_seq"),
        "version": rec.get("updated_at"),
    }


def delete_toss(user_id: str) -> bool:
    store = get_store()
    rec = store.get(CREDENTIALS, user_id) or {}
    if "toss" not in rec:
        return False
    rec.pop("toss")
    store.put(CREDENTIALS, user_id, rec)
    return True


def public_view(user_id: str) -> dict[str, Any]:
    rec = (get_store().get(CREDENTIALS, user_id) or {}).get("toss")
    if not rec:
        return {"configured": False}
    return {
        "configured": True,
        "client_id_masked": rec["client_id_masked"],
        "account_seq": rec.get("account_seq"),
        "verified_at": rec.get("verified_at"),
    }
