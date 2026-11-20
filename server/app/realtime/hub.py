"""Relay Toss real-time market data (WebSocket) to browsers (Server-Sent Events).

Why a relay: the Toss socket needs the user's bearer token and an allow-listed IP, so browsers can't
connect directly — and Toss allows only 2 connections per account. So per user (per server process) we
keep ONE upstream connection carrying the union of everything that user's open tabs want.

    browser tab --SSE--> Subscriber --\
    browser tab --SSE--> Subscriber ----> UserStream (1 Toss WebSocket per user) <---- Toss
                                      /
Orderbooks arrive ~20/s per symbol: subscribers keep only the latest per symbol and the SSE loop flushes
at most 5 times a second. Trades are queued (bounded).

Production: run a single gunicorn worker with threads (gthread) so all tabs of a user share one process.
"""
from __future__ import annotations

import json
import logging
import threading
import time
from collections import deque
from typing import Any, Callable, Iterator, Optional

log = logging.getLogger(__name__)

WS_URL = "wss://openapi-ws.tossinvest.com/ws/v1"
MAX_CODES = 100  # Toss: subscriptions per connection
PING_EVERY = 60  # Toss closes after 180s without client frames
IDLE_STOP = 30  # close upstream this long after the last subscriber leaves
FLUSH_INTERVAL = 0.2  # SSE: at most 5 flushes/s
KEEPALIVE = 15  # SSE comment to detect closed browsers
TOKEN_ERRORS = {"auth-revoked", "token-revoked", "expired-token", "invalid-token"}


def _num(d: dict) -> dict:
    out: dict[str, Any] = {}
    for k, v in d.items():
        if isinstance(v, list):
            out[k] = [_num(x) if isinstance(x, dict) else x for x in v]
        elif isinstance(v, str) and k in ("price", "volume"):
            try:
                out[k] = float(v)
            except ValueError:
                out[k] = v
        else:
            out[k] = v
    return out


class Subscriber:
    """One browser connection. Thread-safe mailbox drained by the SSE generator."""

    def __init__(self, topics: set):
        self.topics = topics
        self._wake = threading.Event()
        self._lock = threading.Lock()
        self._books: dict = {}
        self._trades: deque = deque(maxlen=300)
        self._orders: list = []
        self._status: Optional[dict] = None

    def push(self, topic: str, data: dict) -> None:
        with self._lock:
            if topic.startswith("orderbook:"):
                self._books[topic] = data  # only the latest snapshot matters
            elif topic.startswith("personal:"):
                self._orders.append(data)  # never dropped: every order event matters
            else:
                self._trades.append(data)
        self._wake.set()

    def push_status(self, status: dict) -> None:
        with self._lock:
            self._status = status
        self._wake.set()

    def drain(self) -> list:
        with self._lock:
            out = []
            if self._status is not None:
                out.append(("status", self._status))
                self._status = None
            out += [("order", o) for o in self._orders]
            out += [("trade", t) for t in self._trades]
            out += [("orderbook", b) for b in self._books.values()]
            self._orders.clear()
            self._trades.clear()
            self._books.clear()
        return out

    def events(self, stop: Callable[[], bool] = lambda: False) -> Iterator[str]:
        """SSE text stream. Blocks; ends when the client disconnects (write fails) or stop() is true."""
        while not stop():
            woke = self._wake.wait(KEEPALIVE)
            self._wake.clear()
            if not woke:
                yield ": keepalive\n\n"
                continue
            for event, data in self.drain():
                yield f"event: {event}\ndata: {json.dumps(data, ensure_ascii=False)}\n\n"
            time.sleep(FLUSH_INTERVAL)


class UserStream:
    """Owns one upstream Toss WebSocket for a user and fans messages out to subscribers."""

    def __init__(self, uid: str, token: Callable[[Optional[str]], str], connect: Callable[[str], Any],
                 on_stop: Callable[["UserStream"], None]):
        self.uid = uid
        self._token = token
        self._connect = connect
        self._on_stop = on_stop
        self._subs: set = set()
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._empty_since: Optional[float] = None
        self.status: dict = {"state": "connecting"}
        self.thread = threading.Thread(target=self._run, name=f"toss-ws-{uid[:8]}", daemon=True)

    # ------------------------------------------------------------ subscribers
    def add(self, sub: Subscriber) -> None:
        with self._lock:
            self._subs.add(sub)
            self._empty_since = None
        sub.push_status(self.status)

    def remove(self, sub: Subscriber) -> None:
        with self._lock:
            self._subs.discard(sub)
            if not self._subs:
                self._empty_since = time.time()

    def declaration(self) -> list:
        """Full subscription set (Toss replaces all subscriptions with each declaration)."""
        with self._lock:
            topics = sorted({t for s in self._subs for t in s.topics})
        groups: dict = {}
        for t in topics[:MAX_CODES]:
            kind, market, code = t.split(":", 2)  # trade:kr:005930
            groups.setdefault(f"{kind}:{market}", []).append(code)
        return [{"type": k, "codes": v} for k, v in sorted(groups.items())]

    def _set_status(self, state: str, detail: str = "") -> None:
        self.status = {"state": state, **({"detail": detail} if detail else {})}
        with self._lock:
            subs = list(self._subs)
        for s in subs:
            s.push_status(self.status)

    def _dispatch(self, topic: str, data: dict) -> None:
        payload = {"topic": topic, "symbol": topic.rsplit(":", 1)[-1], **_num(data)}
        with self._lock:
            subs = [s for s in self._subs if topic in s.topics]
        for s in subs:
            s.push(topic, payload)

    # -------------------------------------------------------------------- run
    def start(self) -> None:
        self.thread.start()

    def stop(self) -> None:
        self._stop.set()

    def _idle_expired(self) -> bool:
        with self._lock:
            return self._empty_since is not None and time.time() - self._empty_since > IDLE_STOP

    def _run(self) -> None:
        backoff, rejected, seq = 1.0, None, 0
        try:
            while not self._stop.is_set() and not self._idle_expired():
                ws = None
                token = None
                try:
                    token = self._token(rejected)
                    rejected = None
                    ws = self._connect(token)
                    self._set_status("connected")
                    backoff = 1.0
                    sent, last_ping, last_declare = None, time.time(), 0.0
                    while not self._stop.is_set() and not self._idle_expired():
                        decl = self.declaration()
                        if decl != sent and time.time() - last_declare >= 0.25:  # Toss: <=5 declarations/s
                            seq += 1
                            ws.send(json.dumps([{"id": f"mt-{seq}"}, *decl]))
                            sent, last_declare = decl, time.time()
                        if time.time() - last_ping > PING_EVERY:
                            ws.send("PING")
                            last_ping = time.time()
                        try:
                            raw = ws.recv()
                        except Exception as e:  # recv timeout -> loop again
                            if _is_timeout(e):
                                continue
                            raise
                        if not raw:
                            raise ConnectionError("upstream closed")
                        msg = json.loads(raw)
                        kind = msg.get("type")
                        if kind == "message" and isinstance(msg.get("data"), dict):
                            self._dispatch(msg.get("topic", ""), msg["data"])
                        elif kind == "error":
                            code = (msg.get("error") or {}).get("code", "")
                            if code in TOKEN_ERRORS:
                                rejected = token
                                raise ConnectionError(code)
                            if code == "rate-limit-exceeded":
                                time.sleep(1)
                                sent = None  # re-declare
                            log.warning("toss ws error for %s: %s", self.uid[:8], msg.get("error"))
                        elif kind == "subscriptions" and msg.get("rejected"):
                            log.warning("toss ws rejected topics: %s", msg["rejected"])
                except Exception as e:  # noqa: BLE001 - keep the relay alive, report state to clients
                    if self._stop.is_set():
                        break
                    log.info("toss ws reconnect for %s in %.0fs: %s", self.uid[:8], backoff, e)
                    self._set_status("reconnecting", str(e)[:200])
                    self._stop.wait(backoff)
                    backoff = min(backoff * 2, 30)
                finally:
                    if ws is not None:
                        try:
                            ws.close()
                        except Exception:  # noqa: BLE001
                            pass
        finally:
            self._on_stop(self)


def _is_timeout(e: Exception) -> bool:
    if isinstance(e, TimeoutError):
        return True
    try:
        from websocket import WebSocketTimeoutException
        return isinstance(e, WebSocketTimeoutException)
    except ImportError:  # pragma: no cover
        return False


def default_connect(token: str):
    import websocket  # websocket-client
    return websocket.create_connection(WS_URL, header=[f"Authorization: Bearer {token}"], timeout=1)


class Hub:
    """Process-wide registry: user id -> UserStream."""

    def __init__(self, token_for: Callable[[str, Optional[str]], str], connect: Callable[[str], Any] = default_connect):
        self._token_for = token_for
        self._connect = connect
        self._streams: dict = {}
        self._lock = threading.Lock()

    def subscribe(self, uid: str, topics: set) -> Subscriber:
        sub = Subscriber(topics)
        with self._lock:
            stream = self._streams.get(uid)
            if stream is None:
                stream = UserStream(uid, lambda rejected: self._token_for(uid, rejected), self._connect, self._stopped)
                self._streams[uid] = stream
                stream.add(sub)
                stream.start()
                return sub
        stream.add(sub)
        return sub

    def unsubscribe(self, uid: str, sub: Subscriber) -> None:
        with self._lock:
            stream = self._streams.get(uid)
        if stream:
            stream.remove(sub)

    def _stopped(self, stream: UserStream) -> None:
        with self._lock:
            if self._streams.get(stream.uid) is stream:
                del self._streams[stream.uid]
            with stream._lock:
                leftover = list(stream._subs)
            if leftover and stream.uid not in self._streams:
                # a tab subscribed while this stream was shutting down: hand it to a fresh stream
                fresh = UserStream(stream.uid, lambda rejected: self._token_for(stream.uid, rejected), self._connect, self._stopped)
                self._streams[stream.uid] = fresh
                for sub in leftover:
                    fresh.add(sub)
                fresh.start()

    def active_users(self) -> int:
        with self._lock:
            return len(self._streams)
