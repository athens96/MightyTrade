"""Tests for MT-10: Inbound TradingView / external signal webhook trading bot."""
import time

from app.account import toss_account
from app.autotrade import executor, webhook
from app.quant.engine import Costs
from app.trading import orders as trading
from fakes import FakeToss, login


def test_webhook_bot_secret_auth_idempotency_paper_semi_live_and_rate_limit(client, monkeypatch):
    webhook.reset_rate_limits()
    totp = login(client, monkeypatch, sub="wh_u")
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})
    monkeypatch.setattr("app.autotrade.webhook.credentials.get_toss", lambda uid: {"account_seq": 7})
    monkeypatch.setattr(executor, "_costs", lambda m: Costs(0, 0, 0))

    # 1. Create webhook bot — encrypted secret is never leaked in public response
    r = client.post("/api/autotrade/bots", json={
        "kind": "webhook",
        "market": "KR",
        "symbol": "005930",
        "capital": 200_000,
    })
    assert r.status_code == 200
    bot = r.json["bot"]
    bid = bot["id"]
    assert "webhook_secret_enc" not in bot
    assert bot["webhook_secret_hint"]

    # Retrieve secret via authenticated owner endpoint
    sec_res = client.post(f"/api/autotrade/bots/{bid}/webhook-secret", json={"rotate": False})
    assert sec_res.status_code == 200
    secret = sec_res.json["secret"]

    # 2. Unauthenticated client (no session cookie!) with wrong secret -> 403
    anon = client.application.test_client()
    bad = anon.post(f"/api/autotrade/webhook/{bid}", json={"secret": "wrong", "action": "buy", "price": 10000})
    assert bad.status_code == 403

    # Disabled bot -> 409
    off = anon.post(f"/api/autotrade/webhook/{bid}", json={"secret": secret, "action": "buy", "price": 10000})
    assert off.status_code == 409

    # 3. Enable in paper mode -> buy + idempotency dedupe + exit
    client.post(f"/api/autotrade/bots/{bid}/enable")
    buy1 = anon.post(
        f"/api/autotrade/webhook/{bid}",
        headers={"X-Webhook-Secret": secret},
        json={"symbol": "KRX:005930", "action": "buy", "price": 10000, "idempotency_key": "bar-1"},
    )
    assert buy1.status_code == 200 and buy1.json["qty"] == 20

    # Duplicate bar-1 retry is safely deduplicated without buying again
    dup = anon.post(
        f"/api/autotrade/webhook/{bid}",
        headers={"X-Webhook-Secret": secret},
        json={"action": "buy", "price": 10000, "idempotency_key": "bar-1"},
    )
    assert dup.status_code == 200 and dup.json.get("deduped") is True

    # Exit closes all 20 paper shares at 11000 -> +20,000 profit
    ex = anon.post(
        f"/api/autotrade/webhook/{bid}",
        headers={"X-Webhook-Secret": secret},
        json={"action": "exit", "price": 11000, "idempotency_key": "bar-2"},
    )
    assert ex.status_code == 200 and ex.json["qty"] == 20
    b_state = client.get(f"/api/autotrade/bots/{bid}").json["bot"]["state"]
    assert b_state["position"]["qty"] == 0
    assert b_state["cash"] == 220_000

    # 4. Switch to LIVE mode -> inbound webhook places real Toss order within capital ceiling
    fake = FakeToss({
        ("GET", "/api/v1/prices"): [{"symbol": "005930", "lastPrice": "10000"}],
        ("GET", "/api/v1/buying-power"): {"currency": "KRW", "cashBuyingPower": "1000000"},
        ("POST", "/api/v1/orders"): lambda p, j: {"orderId": f"wh-ord-{j['clientOrderId'][:6]}"},
    })
    for mod in (webhook, executor, trading, toss_account):
        monkeypatch.setattr(mod, "_account", lambda: (fake, 7))

    client.post(f"/api/autotrade/bots/{bid}/mode", json={"mode": "live", "otp_code": totp.at(time.time() + 30)})
    client.post(f"/api/autotrade/bots/{bid}/enable")

    live_buy = anon.post(
        f"/api/autotrade/webhook/{bid}",
        headers={"X-Webhook-Secret": secret},
        json={"action": "buy", "qty_pct": 50, "idempotency_key": "live-1"},
    )
    assert live_buy.status_code == 200
    # 50% of 200,000 capital = 100,000 -> floor(100000 / (10000 * 1.005)) = 9 shares!
    assert live_buy.json["qty"] == 9
    posts = [c for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/orders"]
    assert len(posts) == 1 and posts[0][3]["quantity"] == "9"

    # 5. Global kill-switch blocks inbound webhook immediately
    client.post("/api/autotrade/halt")
    halted = anon.post(f"/api/autotrade/webhook/{bid}", headers={"X-Webhook-Secret": secret}, json={"action": "buy"})
    assert halted.status_code == 409

    # 6. Rate limiter rejects flood (> 15 calls / 60s) with 429
    webhook.reset_rate_limits()
    statuses = [
        anon.post(f"/api/autotrade/webhook/{bid}", json={"secret": "wrong", "action": "buy"}).status_code
        for _ in range(16)
    ]
    assert statuses[-1] == 429
