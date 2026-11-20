import json
import queue
import threading
import time

import pytest

from app.realtime import hub as hubmod
from app.realtime.hub import Hub, Subscriber


class FakeWS:
    """Upstream double: test pushes frames into .incoming; sent frames are recorded."""

    def __init__(self, token):
        self.token = token
        self.sent = []
        self.incoming: queue.Queue = queue.Queue()
        self.closed = False

    def send(self, text):
        self.sent.append(text)

    def recv(self):
        try:
            item = self.incoming.get(timeout=0.05)
        except queue.Empty:
            raise TimeoutError
        if isinstance(item, Exception):
            raise item
        return item

    def close(self):
        self.closed = True


@pytest.fixture
def fast(monkeypatch):
    monkeypatch.setattr(hubmod, "IDLE_STOP", 0.3)
    monkeypatch.setattr(hubmod, "FLUSH_INTERVAL", 0.0)


def make_hub():
    conns, tokens = [], []

    def connect(token):
        ws = FakeWS(token)
        conns.append(ws)
        return ws

    def token_for(uid, rejected):
        tokens.append(rejected)
        return f"tok{len(tokens)}"
    return Hub(token_for, connect), conns, tokens


def wait_for(cond, timeout=2.0):
    end = time.time() + timeout
    while time.time() < end:
        if cond():
            return True
        time.sleep(0.01)
    return False


def msg(topic, data):
    return json.dumps({"type": "message", "topic": topic, "data": data})


def test_one_upstream_per_user_with_union_of_topics(fast):
    hub, conns, _ = make_hub()
    a = hub.subscribe("u1", {"trade:kr:005930", "orderbook:kr:005930"})
    b = hub.subscribe("u1", {"trade:us:AAPL"})
    assert wait_for(lambda: conns and any("AAPL" in s for s in conns[0].sent))
    assert len(conns) == 1  # Toss allows 2 connections per account: tabs share one
    decl = json.loads(conns[0].sent[-1])
    assert {"type": "trade:kr", "codes": ["005930"]} in decl and {"type": "trade:us", "codes": ["AAPL"]} in decl
    assert {"type": "orderbook:kr", "codes": ["005930"]} in decl
    hub.unsubscribe("u1", a)
    hub.unsubscribe("u1", b)


def test_dispatch_routes_by_topic_and_coalesces_orderbooks(fast):
    hub, conns, _ = make_hub()
    kr = hub.subscribe("u1", {"trade:kr:005930", "orderbook:kr:005930"})
    us = hub.subscribe("u1", {"trade:us:AAPL"})
    assert wait_for(lambda: bool(conns))
    for i in range(5):
        conns[0].incoming.put(msg("orderbook:kr:005930", {"asks": [{"price": str(100 + i), "volume": "1"}], "bids": []}))
    conns[0].incoming.put(msg("trade:kr:005930", {"price": "182300", "volume": "8", "timestamp": "t", "currency": "KRW"}))
    assert wait_for(lambda: conns[0].incoming.empty())
    time.sleep(0.1)  # let the relay thread dispatch the last frame
    events = kr.drain()
    books = [d for e, d in events if e == "orderbook"]
    trades = [d for e, d in events if e == "trade"]
    assert len(books) == 1 and books[0]["asks"][0]["price"] == 104.0  # 5 snapshots -> only the latest kept
    assert trades == [{"topic": "trade:kr:005930", "symbol": "005930", "price": 182300.0, "volume": 8.0,
                       "timestamp": "t", "currency": "KRW"}]  # strings -> numbers
    assert [e for e, _ in us.drain() if e in ("trade", "orderbook")] == []  # AAPL tab got nothing for 005930


def test_token_error_reconnects_with_refreshed_token(fast):
    hub, conns, tokens = make_hub()
    sub = hub.subscribe("u1", {"trade:kr:005930"})
    assert wait_for(lambda: bool(conns))
    conns[0].incoming.put(json.dumps({"type": "error", "error": {"code": "auth-revoked"}}))
    assert wait_for(lambda: len(conns) >= 2, timeout=5)
    assert tokens[:2] == [None, "tok1"]  # second fetch tells the client which token was rejected
    assert conns[0].closed
    hub.unsubscribe("u1", sub)


def test_upstream_closes_after_last_subscriber_leaves(fast):
    hub, conns, _ = make_hub()
    sub = hub.subscribe("u1", {"trade:kr:005930"})
    assert wait_for(lambda: bool(conns))
    hub.unsubscribe("u1", sub)
    assert wait_for(lambda: hub.active_users() == 0, timeout=3)
    assert conns[0].closed


def test_sse_text_format(fast):
    s = Subscriber({"trade:kr:005930"})
    s.push("trade:kr:005930", {"price": 1.0})
    gen = s.events(stop=lambda: False)
    chunk = next(gen)
    assert chunk.startswith("event: trade\ndata: ") and chunk.endswith("\n\n")
