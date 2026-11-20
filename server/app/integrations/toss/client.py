"""Toss Securities Open API client (https://developers.tossinvest.com).

Shared by market data (market/providers/toss.py) and, later, orders (trading/brokers/toss.py).
Each account uses its own Toss key (users/credentials.py); there is no server-wide key.

Token rules from the Toss docs that shape this code:
- One valid token per client_id: issuing a new one revokes the previous one immediately.
  So every gunicorn worker / scheduler process must share a single token. It is cached
  encrypted in the Store and refreshed under a cross-process lock.
- No refresh token: re-issue via /oauth2/token when expired (~24h).
"""
from __future__ import annotations

import hashlib
import logging
import threading
import time
from typing import Any, Optional

import requests
from flask import current_app, g

from ...crypto import decrypt, encrypt
from ...errors import ApiError
from ...storage import Store
from ...users import credentials

log = logging.getLogger(__name__)

TOKENS = "integration_tokens"
TOKEN_MARGIN_SEC = 300  # refresh 5 min before expiry
TOKEN_ERRORS = {"invalid-token", "expired-token", "token-revoked"}
MAX_RETRY_AFTER_SEC = 3.0


class TossClient:
    def __init__(
        self,
        base_url: str,
        client_id: str,
        client_secret: str,
        store: Store,
        timeout: float = 10,
        session: Optional[requests.Session] = None,
    ):
        if not client_id or not client_secret:
            raise ApiError(500, "toss_not_configured", "TOSS_CLIENT_ID / TOSS_CLIENT_SECRET are not set")
        self.base_url = base_url.rstrip("/")
        self.client_id = client_id
        self.client_secret = client_secret
        self.store = store
        self.timeout = timeout
        self.session = session or requests.Session()
        # Store key per client_id so a key rotation never reuses a stale token.
        self._token_id = _token_id(client_id)
        self._mem_lock = threading.Lock()
        self._token: Optional[str] = None
        self._expires_at = 0.0

    # ---------------------------------------------------------------- token
    def _issue_token(self) -> tuple[str, float]:
        try:
            r = self.session.post(
                f"{self.base_url}/oauth2/token",
                data={
                    "grant_type": "client_credentials",
                    "client_id": self.client_id,
                    "client_secret": self.client_secret,
                },
                timeout=self.timeout,
            )
        except requests.RequestException as e:
            raise ApiError(502, "toss_unreachable", str(e)) from e
        if r.status_code != 200:
            body = _json(r)
            code = body.get("error") if isinstance(body.get("error"), str) else "token_error"
            log.error("Toss token issue failed: %s %s", r.status_code, body)
            # 403 access_denied = this server's IP is not in the Toss allowlist.
            raise ApiError(502, f"toss_auth_{code}", body.get("error_description") or "Toss token issue failed")
        body = r.json()
        return body["access_token"], time.time() + int(body.get("expires_in", 3600))

    def _load_shared(self) -> tuple[Optional[str], float]:
        rec = self.store.get(TOKENS, self._token_id)
        if not rec:
            return None, 0.0
        try:
            return decrypt(rec["token_enc"]), float(rec["expires_at"])
        except Exception:  # key rotated / corrupted -> treat as missing
            return None, 0.0

    def _get_token(self, force_refresh: bool = False, rejected: Optional[str] = None) -> str:
        with self._mem_lock:
            if not force_refresh and self._token and time.time() < self._expires_at - TOKEN_MARGIN_SEC:
                return self._token
            with self.store.lock(self._token_id):
                token, exp = self._load_shared()
                usable = token and time.time() < exp - TOKEN_MARGIN_SEC
                # Another process may already have refreshed it: reuse unless it's the one Toss just rejected.
                if not usable or (force_refresh and token == rejected):
                    token, exp = self._issue_token()
                    self.store.put(TOKENS, self._token_id, {"token_enc": encrypt(token), "expires_at": exp})
                self._token, self._expires_at = token, exp
                return token

    def access_token(self, rejected: Optional[str] = None) -> str:
        """Shared token for other transports (WebSocket). Pass the token Toss rejected to force a refresh."""
        return self._get_token(force_refresh=rejected is not None, rejected=rejected)

    # -------------------------------------------------------------- request
    def request(
        self,
        method: str,
        path: str,
        *,
        params: Optional[dict] = None,
        json: Any = None,
        account_seq: Optional[int] = None,
    ) -> Any:
        """Call a Toss API and return the `result` payload. Raises ApiError on failure."""
        token = self._get_token()
        for attempt in range(3):
            headers = {"Authorization": f"Bearer {token}"}
            if account_seq is not None:
                headers["X-Tossinvest-Account"] = str(account_seq)
            try:
                r = self.session.request(
                    method, f"{self.base_url}{path}", params=params, json=json,
                    headers=headers, timeout=self.timeout,
                )
            except requests.RequestException as e:
                raise ApiError(502, "toss_unreachable", str(e)) from e

            if r.status_code < 300:
                return _json(r).get("result") if r.content else None

            err = _json(r).get("error") or {}
            code = err.get("code", "") if isinstance(err, dict) else str(err)
            message = err.get("message", "") if isinstance(err, dict) else ""

            if r.status_code == 401 and code in TOKEN_ERRORS and attempt == 0:
                token = self._get_token(force_refresh=True, rejected=token)
                continue
            if r.status_code == 429 and attempt < 2:
                wait = min(float(r.headers.get("Retry-After") or 1), MAX_RETRY_AFTER_SEC)
                time.sleep(wait)
                continue
            raise _map_error(r.status_code, code, message, err)
        raise ApiError(502, "toss_retry_exhausted")


def _token_id(client_id: str) -> str:
    return "toss_" + hashlib.sha256(client_id.encode()).hexdigest()[:16]


def _json(r: requests.Response) -> dict:
    try:
        data = r.json()
        return data if isinstance(data, dict) else {"result": data}
    except ValueError:
        return {}


def _map_error(status: int, code: str, message: str, err: Any) -> ApiError:
    log.warning("Toss API error %s %s %s (requestId=%s)", status, code, message,
                err.get("requestId") if isinstance(err, dict) else None)
    code = code or "unknown"
    # Toss auth problems are a server configuration issue, not the user's session:
    # never return 401 here or the client would think the user was logged out.
    if status in (401, 403):
        return ApiError(502, f"toss_{code}", message or "Toss authentication failed")
    if status == 429:
        return ApiError(429, "toss_rate_limited", message)
    if status >= 500:
        return ApiError(502, f"toss_{code}", message or "Toss server error")
    return ApiError(status, f"toss_{code}", message)  # 400/404/409/422: pass through


def _new_client(client_id: str, client_secret: str) -> TossClient:
    cfg = current_app.config
    return TossClient(
        cfg["TOSS_API_BASE_URL"], client_id, client_secret,
        store=current_app.extensions["store"], timeout=cfg["TOSS_TIMEOUT_SEC"],
    )


def get_toss_client(user_id: Optional[str] = None) -> TossClient:
    """Client bound to one account's own Toss key (defaults to the logged-in user).

    Cached per process and keyed by credential version, so re-registering a key takes effect
    immediately. The access token itself is shared across processes via the Store.
    """
    uid = user_id or g.user["id"]
    creds = credentials.get_toss(uid)
    if not creds:
        raise ApiError(409, "toss_credentials_required", "토스증권 API 키를 먼저 등록하세요 (설정)")
    cache: dict = current_app.extensions.setdefault("toss_clients", {})
    hit = cache.get(uid)
    if hit is None or hit[0] != creds["version"]:
        hit = (creds["version"], _new_client(creds["client_id"], creds["client_secret"]))
        cache[uid] = hit
    return hit[1]


def forget_client(user_id: str, client_id: Optional[str] = None) -> None:
    """Drop the cached client and (if client_id given) the stored access token for that key."""
    current_app.extensions.setdefault("toss_clients", {}).pop(user_id, None)
    if client_id:
        current_app.extensions["store"].delete(TOKENS, _token_id(client_id))


VERIFY_ERRORS = {
    "toss_auth_invalid_client": "client_id 또는 client_secret이 올바르지 않습니다.",
    "toss_auth_access_denied": "토스증권 허용 IP에 이 서버의 IP가 등록되어 있지 않습니다.",
    "toss_ip-not-allowed": "토스증권 허용 IP에 이 서버의 IP가 등록되어 있지 않습니다.",
}


def verify_credentials(client_id: str, client_secret: str) -> Optional[int]:
    """Issue a token with the given key and read the account list. Returns the brokerage accountSeq.

    Errors caused by the user's input (bad key, IP not allowlisted) become 400 with a clear message.
    """
    client = _new_client(client_id, client_secret)
    try:
        accounts = client.request("GET", "/api/v1/accounts") or []
    except ApiError as e:
        if e.code in VERIFY_ERRORS or e.code.startswith("toss_auth_"):
            raise ApiError(400, "toss_verify_failed", VERIFY_ERRORS.get(e.code, e.message)) from e
        raise
    brokerage = [a for a in accounts if a.get("accountType") == "BROKERAGE"] or accounts
    return int(brokerage[0]["accountSeq"]) if brokerage else None
