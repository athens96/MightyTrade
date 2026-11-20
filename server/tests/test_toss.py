"""Toss integration tests against a fake HTTP session (no network)."""
import calendar
import json as jsonlib
from datetime import date

import pytest
from flask import g

from app.errors import ApiError
from app.integrations.toss.client import TossClient
from app.market.aggregate import resample_weekly
from app.market.models import Candle, Interval, Market
from app.market.providers.toss import TossProvider


class FakeResp:
    def __init__(self, status=200, body=None, headers=None):
        self.status_code = status
        self._body = body if body is not None else {}
        self.headers = headers or {}
        self.content = jsonlib.dumps(self._body).encode()

    def json(self):
        return self._body


class FakeSession:
    """Routes (method, path) -> list of responses (consumed in order; last one repeats)."""

    def __init__(self, routes):
        self.routes = {k: list(v) for k, v in routes.items()}
        self.calls = []
        self.token_count = 0

    def _next(self, method, path):
        queue = self.routes[(method, path)]
        return queue.pop(0) if len(queue) > 1 else queue[0]

    def post(self, url, data=None, timeout=None):
        self.token_count += 1
        self.calls.append(("POST", url, data))
        return FakeResp(200, {"access_token": f"tok{self.token_count}", "token_type": "Bearer", "expires_in": 86400})

    def request(self, method, url, params=None, json=None, headers=None, timeout=None):
        path = url.replace("https://toss.test", "")
        self.calls.append((method, path, params, headers))
        return self._next(method, path)


def make_client(app, session):
    return TossClient("https://toss.test", "id", "secret", app.extensions["store"], session=session)


def candle(ts, o, h, l, c, v=1):
    return {"timestamp": ts, "openPrice": str(o), "highPrice": str(h), "lowPrice": str(l),
            "closePrice": str(c), "volume": str(v), "currency": "KRW"}


def ok(result):
    return FakeResp(200, {"result": result})


# ------------------------------------------------------------------ client
def test_token_is_shared_across_clients(app):
    with app.app_context():
        s = FakeSession({("GET", "/api/v1/prices"): [ok([])]})
        make_client(app, s).request("GET", "/api/v1/prices")
        # A second process (new client, same store) must reuse the stored token, not revoke it.
        make_client(app, s).request("GET", "/api/v1/prices")
        assert s.token_count == 1
        assert s.calls[-1][3]["Authorization"] == "Bearer tok1"


def test_revoked_token_is_reissued_once(app):
    with app.app_context():
        revoked = FakeResp(401, {"error": {"code": "token-revoked", "message": "revoked"}})
        s = FakeSession({("GET", "/api/v1/prices"): [revoked, ok([{"symbol": "005930"}])]})
        assert make_client(app, s).request("GET", "/api/v1/prices") == [{"symbol": "005930"}]
        assert s.token_count == 2
        assert s.calls[-1][3]["Authorization"] == "Bearer tok2"


def test_toss_auth_error_is_not_reported_as_user_401(app):
    with app.app_context():
        s = FakeSession({("GET", "/api/v1/prices"): [FakeResp(403, {"error": {"code": "ip-not-allowed"}})]})
        with pytest.raises(ApiError) as e:
            make_client(app, s).request("GET", "/api/v1/prices")
        assert e.value.status == 502 and e.value.code == "toss_ip-not-allowed"


def test_rate_limit_retries(app):
    with app.app_context():
        limited = FakeResp(429, {"error": {"code": "rate-limit-exceeded"}}, {"Retry-After": "0"})
        s = FakeSession({("GET", "/api/v1/prices"): [limited, ok([1])]})
        assert make_client(app, s).request("GET", "/api/v1/prices") == [1]


def test_business_errors_pass_through(app):
    with app.app_context():
        s = FakeSession({("GET", "/api/v1/candles"): [FakeResp(404, {"error": {"code": "stock-not-found"}})]})
        with pytest.raises(ApiError) as e:
            make_client(app, s).request("GET", "/api/v1/candles")
        assert e.value.status == 404 and e.value.code == "toss_stock-not-found"


# ---------------------------------------------------------------- provider
@pytest.fixture
def provider(app, monkeypatch):
    """Call inside an app context. Acts as logged-in user u1 with a fake Toss client."""
    def _make(routes):
        session = FakeSession(routes)
        client = make_client(app, session)
        monkeypatch.setattr("app.market.providers.toss.get_toss_client", lambda: client)
        g.user = {"id": "u1"}
        return TossProvider(app.config), session
    return _make


def test_caches_are_per_user(app, provider):
    with app.app_context():
        p, s = provider({("GET", "/api/v1/candles"): [ok({"candles": [
            candle("2026-11-20T00:00:00.000+09:00", 1, 2, 0.5, 1.5)], "nextBefore": None})]})
        p.get_candles(Market.KR, "005930", Interval.D1, 1)
        p.get_candles(Market.KR, "005930", Interval.D1, 1)  # cached for u1
        g.user = {"id": "u2"}
        p.get_candles(Market.KR, "005930", Interval.D1, 1)  # u2 must not get u1's data
        assert len([c for c in s.calls if c[1] == "/api/v1/candles"]) == 2


def test_daily_candles_normalized_to_trading_date(app, provider):
    with app.app_context():
        p, _ = provider({("GET", "/api/v1/candles"): [ok({"candles": [
            candle("2026-11-20T14:00:00.000+09:00", 1, 2, 0.5, 1.5),  # US bar (00:00 ET)
            candle("2026-11-19T14:00:00.000+09:00", 1, 2, 0.5, 1.0),
        ], "nextBefore": None})]})
        out = p.get_candles(Market.US, "AAPL", Interval.D1, 2)
        assert [c.time for c in out] == [calendar.timegm(date(2026, 11, d).timetuple()) for d in (19, 20)]
        assert out[-1].close == 1.5


def test_intraday_resample_with_pagination(app, provider):
    with app.app_context():
        # 10 one-minute bars 09:00..09:09 KST across two pages (newest first)
        bars = [candle(f"2026-11-20T09:{m:02d}:00.000+09:00", 100 + m, 101 + m, 99 + m, 100.5 + m, 10)
                for m in range(9, -1, -1)]
        p, s = provider({("GET", "/api/v1/candles"): [
            ok({"candles": bars[:5], "nextBefore": "2026-11-20T09:04:00+09:00"}),
            ok({"candles": bars[5:], "nextBefore": None}),
        ]})
        out = p.get_candles(Market.KR, "005930", Interval.M5, 2)
        assert len(out) == 2
        first, second = out
        assert (first.open, first.high, first.low, first.close, first.volume) == (100, 105, 99, 104.5, 50)
        assert second.open == 105 and second.close == 109.5
        assert s.calls[-1][2]["before"] == "2026-11-20T09:04:00+09:00"


def test_weekly_resample():
    day = lambda d: calendar.timegm(date(2026, 11, d).timetuple())  # noqa: E731
    daily = [Candle(day(d), d, d + 1, d - 1, d + 0.5, 1) for d in (13, 16, 17, 18, 19, 20)]  # Fri, Mon..Fri
    weeks = resample_weekly(daily)
    assert [w.time for w in weeks] == [day(9), day(16)]
    assert (weeks[1].open, weeks[1].high, weeks[1].close, weeks[1].volume) == (16, 21, 20.5, 5)


def test_quote_change_vs_previous_session(app, provider):
    with app.app_context():
        p, _ = provider({
            ("GET", "/api/v1/prices"): [ok([{"symbol": "005930", "lastPrice": "182400",
                                             "timestamp": "2026-11-20T14:59:21.000+09:00", "currency": "KRW"}])],
            ("GET", "/api/v1/candles"): [ok({"candles": [
                candle("2026-11-20T00:00:00.000+09:00", 190800, 190800, 181400, 182400),
                candle("2026-11-19T00:00:00.000+09:00", 196000, 197600, 189800, 197300),
            ], "nextBefore": "x"})],
        })
        q = p.get_quote(Market.KR, "005930")
        assert q.price == 182400 and q.change == -14900 and q.change_pct == -7.55


def test_symbol_market_mismatch(app, provider):
    with app.app_context():
        p, _ = provider({})
        with pytest.raises(ApiError) as e:
            p.get_candles(Market.KR, "AAPL", Interval.D1)
        assert e.value.code == "symbol_market_mismatch"


def test_search_ranks_symbol_then_name(app, provider, monkeypatch):
    monkeypatch.setattr("app.market.providers.toss.time.sleep", lambda s: None)
    with app.app_context():
        stocks = [
            {"symbol": "005930", "name": "삼성전자", "englishName": "Samsung Electronics", "market": "KOSPI"},
            {"symbol": "005935", "name": "삼성전자우", "englishName": "Samsung Electronics Pref", "market": "KOSPI"},
            {"symbol": "000660", "name": "SK하이닉스", "englishName": "SK hynix", "market": "KOSPI"},
        ]
        p, s = provider({("GET", "/api/v1/stocks/all"): [ok(stocks), ok([]), ok([])]})
        assert [i.symbol for i in p.search(Market.KR, "삼성")] == ["005930", "005935"]
        assert [i.symbol for i in p.search(Market.KR, "000660")] == ["000660"]
        assert [i.symbol for i in p.search(Market.KR, "hynix")] == ["000660"]
        # full list fetched once (3 KR markets), then served from cache
        assert len([c for c in s.calls if c[1] == "/api/v1/stocks/all"]) == 3


def test_stock_list_single_flight_dedupes_concurrent_fetches(app, provider, monkeypatch):
    import threading
    import time as timemod
    monkeypatch.setattr("app.market.providers.toss.time.sleep", lambda s: None)
    with app.app_context():
        stocks = [{"symbol": "005930", "name": "삼성전자", "englishName": "Samsung", "market": "KOSPI"}]
        p, s = provider({("GET", "/api/v1/stocks/all"): [ok(stocks), ok([]), ok([])]})
        orig_req = s.request

        def slow_req(*args, **kwargs):
            timemod.sleep(0.05)
            return orig_req(*args, **kwargs)
        s.request = slow_req

        results = []
        barrier = threading.Barrier(3)

        def worker():
            with app.app_context():
                g.user = {"id": "u1"}
                barrier.wait()
                results.append([i.symbol for i in p.search(Market.KR, "삼성")])

        threads = [threading.Thread(target=worker) for _ in range(3)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        assert results == [["005930"]] * 3
        # 3 concurrent searches on cold cache must trigger only 1 fetch pass (3 KR exchanges)
        assert len([c for c in s.calls if c[1] == "/api/v1/stocks/all"]) == 3


def test_us_search_enriches_english_names_via_stocks_batch(app, provider, monkeypatch):
    monkeypatch.setattr("app.market.providers.toss.time.sleep", lambda s: None)
    with app.app_context():
        us_all = [
            {"symbol": "AAPL", "name": "애플", "market": "NASDAQ"},  # no englishName in /stocks/all
            {"symbol": "NVDA", "name": "엔비디아", "market": "NASDAQ"},
        ]
        us_details = [
            {"symbol": "AAPL", "name": "애플", "englishName": "Apple Inc."},
            {"symbol": "NVDA", "name": "엔비디아", "englishName": "NVIDIA Corporation"},
        ]
        p, s = provider({
            ("GET", "/api/v1/stocks/all"): [ok(us_all), ok([]), ok([]), ok([])],
            ("GET", "/api/v1/stocks"): [ok(us_details)],
        })
        hits = p.search(Market.US, "nvidia")
        assert [i.symbol for i in hits] == ["NVDA"] and hits[0].english_name == "NVIDIA Corporation"
        assert [i.symbol for i in p.search(Market.US, "apple")] == ["AAPL"]
        assert len([c for c in s.calls if c[1] == "/api/v1/stocks"]) == 1


def test_batch_quotes_one_price_call_and_per_symbol_errors(app, provider):
    with app.app_context():
        p, s = provider({
            ("GET", "/api/v1/prices"): [ok([
                {"symbol": "005930", "lastPrice": "182400", "timestamp": "2026-11-20T14:59:21.000+09:00", "currency": "KRW"},
                {"symbol": "000660", "lastPrice": "600000", "timestamp": "2026-11-20T14:59:21.000+09:00", "currency": "KRW"},
            ])],
            ("GET", "/api/v1/candles"): [ok({"candles": [
                candle("2026-11-20T00:00:00.000+09:00", 1, 1, 1, 182400),
                candle("2026-11-19T00:00:00.000+09:00", 1, 1, 1, 197300),
            ], "nextBefore": "x"})],
        })
        out = p.get_quotes(Market.KR, ["005930", "000660", "999999", "AAPL"])
        price_calls = [c for c in s.calls if c[1] == "/api/v1/prices"]
        assert len(price_calls) == 1 and price_calls[0][2]["symbols"] == "005930,000660,999999"
        assert out["005930"].price == 182400 and out["005930"].change == -14900
        assert isinstance(out["999999"], ApiError) and out["999999"].status == 404  # not returned by Toss
        assert out["AAPL"].code == "symbol_market_mismatch"  # never sent to Toss
