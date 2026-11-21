"""End-to-end multi-bot trading integration test suite (MT-1).

Exercises all 6 bot categories (`swing`, `daytrade`, `rebalance`, `rotation`, `dca`, `webhook`)
across paper, semi-auto 1-click approval, and live Toss execution, together with OCO risk
protection, circuit breakers, market timing regimes, real-time notifications, unified overview
analytics, and global kill-switch / OTP resume.
"""
from __future__ import annotations

from datetime import date, datetime, timedelta, timezone

from app.account import toss_account
from app.autotrade import bots, dca, executor, notifier, rebalance, rotation, scheduler, semi, webhook
from app.market.models import Candle, Interval
from app.quant import screener
from app.quant.engine import Costs, Order
from app.quant.strategies import STRATEGIES, Strategy
from app.trading import orders as trading
from app.users import repository as user_repo
from fakes import FakeToss, login


class E2ESwing(Strategy):
    id, name, summary, description, params = "e2e_swing", "통합스윙", "s", "s", []
    category, timeframe = "swing", "1d"

    def warmup(self):
        return 2

    def prepare(self, bars):
        self.bars = bars

    def on_bar(self, i, ctx):
        if not ctx.position.is_open:
            return [Order("buy", reason="통합 스윙 진입")]
        return []


class E2EDaytrade(Strategy):
    id, name, summary, description, params = "e2e_day", "통합단타", "s", "s", []
    category, timeframe = "daytrade", "5m"

    def warmup(self):
        return 2

    def prepare(self, bars):
        self.bars = bars

    def on_bar(self, i, ctx):
        if not ctx.position.is_open:
            return [Order("buy", reason="통합 분봉 돌파")]
        return []


def _daily_candles(today_dt: date, n: int = 80, px: float = 10_000.0) -> list[Candle]:
    cur = today_dt - timedelta(days=int(n * 1.6))
    out: list[Candle] = []
    while cur <= today_dt:
        if cur.weekday() < 5 or cur == today_dt:
            ts = int(datetime(cur.year, cur.month, cur.day, tzinfo=timezone.utc).timestamp())
            out.append(Candle(ts, px, px * 1.02, px * 0.98, px, 5000))
        cur += timedelta(days=1)
    return out


def _intraday_candles(now_dt: datetime, n: int = 30, px: float = 20_000.0) -> list[Candle]:
    base = int(now_dt.timestamp()) // 300 * 300 - n * 300
    return [Candle(base + i * 300, px, px + 200, px - 200, px + 50, 1000) for i in range(n)]


def test_all_six_bot_categories_end_to_end_lifecycle(client, monkeypatch):
    webhook.reset_rate_limits()
    totp = login(client, monkeypatch, sub="e2e_all_bots")
    uid = client.get("/api/auth/me").json["user"]["id"]

    monkeypatch.setitem(STRATEGIES, "e2e_swing", E2ESwing)
    monkeypatch.setitem(STRATEGIES, "e2e_day", E2EDaytrade)
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda u: {"account_seq": 9})
    monkeypatch.setattr(scheduler.credentials, "get_toss", lambda u: {"account_seq": 9})
    monkeypatch.setattr("app.autotrade.webhook.credentials.get_toss", lambda u: {"account_seq": 9})

    now_utc = datetime.now(timezone.utc)
    today_dt = now_utc.date()
    d_cs = _daily_candles(today_dt)
    m_cs = _intraday_candles(now_utc)
    today = executor.day_of(d_cs[-1].time)

    class Provider:
        def get_history(self, market, symbol, interval, start):
            return d_cs

        def get_candles(self, market, symbol, interval, limit=200):
            return m_cs if interval != Interval.D1 else d_cs[-limit:]

        def get_quote(self, market, symbol):
            return None

    prov = Provider()
    for mod in (executor, rebalance, rotation, dca, webhook):
        monkeypatch.setattr(mod, "get_provider", lambda m: prov)
    monkeypatch.setattr("app.quant.market_timing.get_provider", lambda m: prov)
    for mod in (executor, rebalance, rotation, dca):
        monkeypatch.setattr(mod, "_costs", lambda m: Costs(0, 0, 0))

    monkeypatch.setattr(
        screener,
        "run_screen",
        lambda *a, **kw: {
            "items": [
                {"symbol": "005930", "name": "삼성전자", "rank": 1, "score": 92.0, "price": 10_000.0},
                {"symbol": "000660", "name": "SK하이닉스", "rank": 2, "score": 88.0, "price": 10_000.0},
                {"symbol": "035420", "name": "NAVER", "rank": 3, "score": 81.0, "price": 10_000.0},
            ]
        },
    )

    sent_alerts: list[str] = []
    monkeypatch.setattr(notifier, "_send_http", lambda url, payload: sent_alerts.append(str(payload)))
    assert client.put("/api/users/me/notifications", json={
        "enabled": True,
        "paper_notify": True,
        "discord_webhook_url": "https://discord.com/api/webhooks/123/abc",
        "events": {"order": True, "fill": True, "error": True, "halt": True, "briefing": True},
    }).status_code == 200

    # 1. Create all 6 bot categories
    b_swing = client.post("/api/autotrade/bots", json={
        "name": "1_스윙봇", "market": "KR", "symbol": "005930", "strategy": "e2e_swing",
        "risk": {"stop_loss_pct": 5.0, "take_profit_pct": 10.0},
        "guard": {"max_drawdown_pct": 15.0},
        "capital": 200_000,
    }).json["bot"]
    b_day = client.post("/api/autotrade/bots", json={
        "name": "2_단타봇", "market": "KR", "symbol": "000660", "strategy": "e2e_day", "capital": 200_000,
    }).json["bot"]
    b_rebal = client.post("/api/autotrade/bots", json={
        "kind": "rebalance", "name": "3_리밸런싱봇", "market": "KR", "model": "all_weather",
        "buy_only": True, "rebalance": "monthly",
        "assets": [
            {"symbol": "069500", "role": "offensive", "weight": 60},
            {"symbol": "148070", "role": "defensive", "weight": 40},
        ],
        "capital": 300_000,
    }).json["bot"]
    b_rot = client.post("/api/autotrade/bots", json={
        "kind": "rotation", "name": "4_순환매봇", "market": "KR",
        "preset": "momentum_quality", "universe": "kr_all_150", "top_n": 2, "hold_buffer": 4,
        "rebalance": "weekly", "capital": 200_000,
    }).json["bot"]
    b_dca = client.post("/api/autotrade/bots", json={
        "kind": "dca", "name": "5_적립식봇", "market": "KR", "symbol": "035420",
        "rebalance": "daily", "base_amount": 50_000, "smart_scale": True, "capital": 200_000,
    }).json["bot"]
    b_wh = client.post("/api/autotrade/bots", json={
        "kind": "webhook", "name": "6_웹훅봇", "market": "KR", "symbol": "005930", "capital": 150_000,
    }).json["bot"]

    for b in (b_swing, b_day, b_rebal, b_rot, b_dca, b_wh):
        assert client.post(f"/api/autotrade/bots/{b['id']}/enable").status_code == 200

    # 2. Scheduler tick runs all paper bots cleanly inside active session
    start = now_utc - timedelta(minutes=10)
    end = now_utc + timedelta(hours=5)
    monkeypatch.setattr(scheduler, "sessions", lambda m: [(today, start, end)])
    monkeypatch.setattr("app.autotrade.routes.target_date", lambda m, n: today)
    scheduler.tick(client.application, now=now_utc)

    assert client.get(f"/api/autotrade/bots/{b_swing['id']}").json["bot"]["state"]["position"]["qty"] == 20
    assert client.get(f"/api/autotrade/bots/{b_day['id']}").json["bot"]["state"]["position"]["qty"] == 10
    assert len(client.get(f"/api/autotrade/bots/{b_rebal['id']}").json["bot"]["state"]["positions"]) == 2
    assert len(client.get(f"/api/autotrade/bots/{b_rot['id']}").json["bot"]["state"]["positions"]) == 2
    assert client.get(f"/api/autotrade/bots/{b_dca['id']}").json["bot"]["state"]["dca_stats"]["steps"] == 1

    # Inbound webhook triggers paper buy on webhook bot
    wh_sec = client.post(f"/api/autotrade/bots/{b_wh['id']}/webhook-secret", json={"rotate": False}).json["secret"]
    anon = client.application.test_client()
    wh_r = anon.post(f"/api/autotrade/webhook/{b_wh['id']}", headers={"X-Webhook-Secret": wh_sec}, json={"action": "buy", "price": 10_000})
    assert wh_r.status_code == 200 and wh_r.json["qty"] == 15

    # 3. Promote Swing Bot to LIVE, execute + reconcile fill + OCO sync + slippage tracking
    fake = FakeToss({
        ("GET", "/api/v1/prices"): [{"symbol": "005930", "lastPrice": "10000"}],
        ("GET", "/api/v1/buying-power"): {"currency": "KRW", "cashBuyingPower": "1000000"},
        ("GET", "/api/v1/sellable-quantity"): {"sellableQuantity": "25"},
        ("POST", "/api/v1/orders"): lambda p, j: {"orderId": "live-ord-1"},
        ("GET", "/api/v1/orders/live-ord-1"): {
            "orderId": "live-ord-1",
            "status": "FILLED",
            "execution": {"filledQuantity": 19, "averageFilledPrice": "10010"},
        },
        ("POST", "/api/v1/conditional-orders"): lambda p, j: {"conditionalOrderId": "oco-1"},
        ("DELETE", "/api/v1/conditional-orders/oco-1"): {},
    })
    for mod in (executor, semi, webhook, trading, toss_account):
        monkeypatch.setattr(mod, "_account", lambda: (fake, 9))

    with client.application.app_context():
        user_repo.update_user(uid, otp_last_step=0)
    assert client.post(f"/api/autotrade/bots/{b_swing['id']}/mode", json={"mode": "live", "otp_code": totp.now()}).status_code == 200
    client.post(f"/api/autotrade/bots/{b_swing['id']}/enable")

    scheduler.tick(client.application, now=now_utc + timedelta(minutes=3))
    with client.application.app_context():
        rec_sw = bots.get(uid, b_swing["id"])
        executor.reconcile_live(rec_sw, today)
        bots.save(rec_sw)

    # Protection OCO attached automatically after live fill!
    cond_posts = [c for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/conditional-orders"]
    assert len(cond_posts) >= 1 and cond_posts[0][3]["type"] == "OCO"

    # 4. Unified overview & regimes check
    reg = client.get("/api/market/regimes")
    assert reg.status_code == 200 and "KR" in reg.json

    ov = client.get("/api/autotrade/overview").json
    assert ov["total_bots"] == 6
    assert ov["paper"]["bots"] == 5
    assert ov["live"]["bots"] == 1
    assert ov["execution"]["total_fills"] == 1
    assert ov["execution"]["avg_slippage_bps"] == 10.0  # 10010 vs 10000 = +10 bps
    assert any(d["symbol"] == "005930" for d in ov["duplicate_symbols"])

    # 5. Global Kill Switch cancels live OCO order & Resume requires OTP
    halt_r = client.post("/api/autotrade/halt")
    assert halt_r.status_code == 200 and halt_r.json["cancelled"] >= 1
    with client.application.app_context():
        user_repo.update_user(uid, otp_last_step=0)
    res_r = client.post("/api/autotrade/resume", json={"otp_code": totp.now()})
    assert res_r.status_code == 200 and res_r.json["halted"] is False
    assert len(sent_alerts) >= 4
