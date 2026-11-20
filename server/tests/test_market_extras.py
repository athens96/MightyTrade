"""Toss market extras: session status (KR/US calendars) and index overview."""
from datetime import datetime, timedelta, timezone

import pytest

from app.market import extras
from fakes import FakeToss, login


@pytest.fixture
def session(client, monkeypatch):
    return client, login(client, monkeypatch)


# ------------------------------------------------------------------ session status
KR_CAL = {
    "today": {"date": "2026-11-20", "integrated": {
        "preMarket": {"startTime": "2026-11-20T08:00:00+09:00", "endTime": "2026-11-20T09:00:00+09:00"},
        "regularMarket": {"startTime": "2026-11-20T09:00:00+09:00", "endTime": "2026-11-20T15:30:00+09:00"},
        "afterMarket": {"startTime": "2026-11-20T15:30:00+09:00", "endTime": "2026-11-20T20:00:00+09:00"}}},
    "previousBusinessDay": {"date": "2026-11-19", "integrated": None},
    "nextBusinessDay": {"date": "2026-11-23", "integrated": {
        "preMarket": {"startTime": "2026-11-23T08:00:00+09:00", "endTime": "2026-11-23T09:00:00+09:00"}}},
}
US_CAL = {
    "previousBusinessDay": {"date": "2026-11-19", "afterMarket": {"startTime": "2026-11-20T06:00:00+09:00", "endTime": "2026-11-20T09:50:00+09:00"}},
    "today": {"date": "2026-11-20",
              "dayMarket": {"startTime": "2026-11-20T10:00:00+09:00", "endTime": "2026-11-20T18:00:00+09:00"},
              "regularMarket": {"startTime": "2026-11-20T23:30:00+09:00", "endTime": "2026-11-21T06:00:00+09:00"}},
    "nextBusinessDay": {"date": "2026-11-23"},
}
KST = lambda h, m=0: datetime(2026, 11, 20, h, m, tzinfo=timezone(timedelta(hours=9)))  # noqa: E731


def test_session_status_kr_and_us():
    assert extras.session_status("KR", KR_CAL, KST(10))["label"] == "정규장"
    s = extras.session_status("KR", KR_CAL, KST(16, 40))
    assert s["open"] and s["session"] == "afterMarket"
    closed = extras.session_status("KR", KR_CAL, KST(21))
    assert not closed["open"] and closed["label"] == "장 마감" and closed["next"]["session"] == "preMarket"
    assert extras.session_status("US", US_CAL, KST(16, 40))["label"] == "데이마켓"
    assert extras.session_status("US", US_CAL, KST(8))["session"] == "afterMarket"  # previous day's session, today morning
    assert extras.session_status("US", US_CAL, KST(23, 45))["label"] == "정규장"  # crosses midnight KST
    holiday = {**KR_CAL, "today": {"date": "2026-11-21", "integrated": None}}
    assert extras.session_status("KR", holiday, KST(21))["label"] == "휴장"


def test_overview_index_change(session, app, monkeypatch):
    c, _ = session
    app.config["MARKET_PROVIDERS"] = {"KR": "toss", "US": "toss"}
    fake = FakeToss({
        ("GET", "/api/v1/market-indicators/prices"): [{"symbol": "KOSPI", "lastPrice": "4869.96"}, {"symbol": "KOSDAQ", "lastPrice": "645.68"}],
        ("GET", "/api/v1/market-indicators/KOSPI/candles"): {"candles": [{"closePrice": "4869.96"}, {"closePrice": "5137.89"}]},
        ("GET", "/api/v1/market-indicators/KOSDAQ/candles"): {"candles": [{"closePrice": "645.68"}, {"closePrice": "640"}]},
        ("GET", "/api/v1/market-calendar/KR"): KR_CAL,
        ("GET", "/api/v1/market-calendar/US"): US_CAL,
    })
    monkeypatch.setattr(extras, "get_toss_client", lambda: fake)
    extras._cache.clear()
    body = c.get("/api/market/overview").json
    kospi = body["indices"][0]
    assert kospi["symbol"] == "KOSPI" and round(kospi["change"], 2) == -267.93 and round(kospi["change_pct"], 2) == -5.21
    assert set(body["status"]) == {"KR", "US"}
