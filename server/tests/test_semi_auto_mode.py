"""Tests for MT-8: Semi-automated '1-Click Confirm-to-Trade' ('승인 후 주문') mode."""
import time
from datetime import date, datetime, timedelta, timezone

from app.account import toss_account
from app.autotrade import bots, executor, scheduler, semi
from app.market.models import Candle
from app.quant.engine import Costs, Order
from app.quant.strategies import STRATEGIES, Strategy
from app.trading import orders as trading
from fakes import FakeToss, login


class SemiSig(Strategy):
    id, name, summary, description, params = "semi_sig", "반자동테스트", "s", "s", []

    def warmup(self):
        return 1

    def prepare(self, bars):
        self.bars = bars

    def on_bar(self, i, ctx):
        if not ctx.position.is_open:
            return [Order("buy", reason="반자동 매수 신호")]
        return []


def _candles(today_dt: date):
    cur = today_dt - timedelta(days=25)
    out = []
    while cur <= today_dt:
        if cur.weekday() < 5 or cur == today_dt:
            ts = int(datetime(cur.year, cur.month, cur.day, tzinfo=timezone.utc).timestamp())
            out.append(Candle(ts, 10000, 10100, 9900, 10000, 1000))
        cur += timedelta(days=1)
    return out


def test_semi_mode_stages_proposal_and_submits_only_on_one_click_approval(client, monkeypatch):
    totp = login(client, monkeypatch, sub="semi_u")
    monkeypatch.setitem(STRATEGIES, "semi_sig", SemiSig)
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})
    monkeypatch.setattr(scheduler.credentials, "get_toss", lambda uid: {"account_seq": 7})

    now_utc = datetime.now(timezone.utc)
    cs = _candles(now_utc.date())
    today = executor.day_of(cs[-1].time)
    monkeypatch.setattr(executor, "get_provider", lambda m: type("P", (), {"get_history": lambda self, *a: cs})())
    monkeypatch.setattr(executor, "_costs", lambda m: Costs(0, 0, 0))

    fake = FakeToss({
        ("GET", "/api/v1/prices"): [{"symbol": "005930", "lastPrice": "10000"}],
        ("GET", "/api/v1/buying-power"): {"currency": "KRW", "cashBuyingPower": "500000"},
        ("POST", "/api/v1/orders"): lambda p, j: {"orderId": f"ord-{j['clientOrderId'][:8]}"},
    })
    for mod in (semi, executor, trading, toss_account):
        monkeypatch.setattr(mod, "_account", lambda: (fake, 7))

    # 1. Create bot and switch to 'semi' mode with OTP
    r = client.post("/api/autotrade/bots", json={
        "market": "KR", "symbol": "005930", "strategy": "semi_sig", "capital": 100_000,
    })
    bid = r.json["bot"]["id"]
    m_res = client.post(f"/api/autotrade/bots/{bid}/mode", json={"mode": "semi", "otp_code": totp.at(time.time() + 30)})
    assert m_res.status_code == 200 and m_res.json["bot"]["mode"] == "semi"
    client.post(f"/api/autotrade/bots/{bid}/enable")

    # 2. Scheduler tick stages pending_approval WITHOUT sending any order to Toss!
    start = now_utc - timedelta(minutes=5)
    end = now_utc + timedelta(hours=5)
    monkeypatch.setattr(scheduler, "sessions", lambda m: [(today, start, end)])
    scheduler.tick(client.application, now=now_utc)

    order_posts = [c for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/orders"]
    assert len(order_posts) == 0  # Nothing sent before user approves!

    b_view = client.get(f"/api/autotrade/bots/{bid}").json["bot"]
    assert b_view["state"]["pending_approval"] is not None
    assert b_view["state"]["pending_approval"]["orders"][0]["qty"] == 9  # floor(100000 / (10000 * 1.005)) = 9 shares!

    # 3. Attempting approve while locked without OTP returns 403
    locked = client.post(f"/api/autotrade/bots/{bid}/approve", json={})
    assert locked.status_code == 403

    # 4. 1-Click approve with fresh OTP (after 30s window advances) unlocks & sends the exact staged order once!
    uid = client.get("/api/auth/me").json["user"]["id"]
    from app.users import repository as user_repo
    with client.application.app_context():
        user_repo.update_user(uid, otp_last_step=0)
    ok = client.post(f"/api/autotrade/bots/{bid}/approve", json={"otp_code": totp.now()})
    assert ok.status_code == 200
    assert ok.json["result"]["sent"] == 1
    assert ok.json["bot"]["state"]["pending_approval"] is None
    order_posts = [c for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/orders"]
    assert len(order_posts) == 1
    assert order_posts[0][3]["quantity"] == "9"

    # 5. Expired proposal returns 410 approval_expired (session is already unlocked from step 4)
    with client.application.app_context():
        rec = bots.get(uid, bid)
        rec["state"]["live"]["pending_approval"] = {
            "id": "exp123",
            "date": today,
            "created_at": (now_utc - timedelta(hours=7)).isoformat(),
            "expires_at": (now_utc - timedelta(minutes=1)).isoformat(),
            "reason": "만료 테스트",
            "orders": [{"symbol": "005930", "side": "BUY", "kind": "market", "qty": 1, "signal_price": 10000, "level": None, "reason": "", "text": ""}],
        }
        bots.save(rec)
    assert client.get(f"/api/autotrade/bots/{bid}").json["bot"]["state"]["pending_approval"] is None
    exp_res = client.post(f"/api/autotrade/bots/{bid}/approve", json={})
    assert exp_res.status_code == 410
