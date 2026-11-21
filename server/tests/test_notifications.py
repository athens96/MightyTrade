"""Tests for MT-2: Real-time trade notifications (Telegram/Discord/Slack) & daily close briefing."""
from datetime import datetime, timedelta, timezone

import pytest

from app.autotrade import bots, notifier, scheduler
from app.users import credentials
from fakes import login


def test_notification_settings_encrypt_mask_and_ssrf_guard(client, monkeypatch):
    login(client, monkeypatch, sub="notify_u")

    # Reject private/http webhook URLs (SSRF guard)
    bad = client.put("/api/users/me/notifications", json={
        "enabled": True,
        "paper_notify": False,
        "discord_webhook_url": "http://127.0.0.1:8080/hook",
        "events": {"order": True, "fill": True, "error": True, "halt": True, "briefing": True},
    })
    assert bad.status_code == 400

    # Require chat_id when telegram_bot_token is set
    no_chat = client.put("/api/users/me/notifications", json={
        "enabled": True,
        "telegram_bot_token": "123456:ABCDEF_secret_token",
    })
    assert no_chat.status_code == 400

    # Save valid Telegram + Discord config
    res = client.put("/api/users/me/notifications", json={
        "enabled": True,
        "paper_notify": True,
        "telegram_bot_token": "123456789:AAH_super_secret_token_xyz",
        "telegram_chat_id": "99887766",
        "discord_webhook_url": "https://discord.com/api/webhooks/111/secret_hook_token",
        "events": {"order": True, "fill": True, "error": False, "halt": True, "briefing": True},
    })
    assert res.status_code == 200
    body = res.json
    assert body["configured"] is True
    assert body["telegram_configured"] is True
    assert "super_secret" not in str(body)
    assert "secret_hook" not in str(body)
    assert body["events"]["error"] is False

    # Updating toggles with __KEEP__ preserves secrets
    keep = client.put("/api/users/me/notifications", json={
        "enabled": True,
        "paper_notify": False,
        "telegram_bot_token": "__KEEP__",
        "telegram_chat_id": "99887766",
        "discord_webhook_url": "__KEEP__",
        "events": {"order": True, "fill": True, "error": True, "halt": True, "briefing": True},
    })
    assert keep.status_code == 200
    assert keep.json["telegram_configured"] is True and keep.json["discord_configured"] is True

    # Test send dispatches to both Telegram and Discord
    calls = []
    monkeypatch.setattr(notifier, "_send_http", lambda url, payload: calls.append((url, payload)))
    sent = client.post("/api/users/me/notifications/test")
    assert sent.status_code == 200
    assert set(sent.json["sent"]) == {"telegram", "discord"}
    assert len(calls) == 2

    # Delete clears everything
    deleted = client.delete("/api/users/me/notifications")
    assert deleted.status_code == 200 and deleted.json["configured"] is False


def test_bot_events_and_daily_close_briefing_dispatch(app, monkeypatch):
    sent_msgs = []
    monkeypatch.setattr(notifier, "_send_http", lambda url, payload: sent_msgs.append((url, payload)))

    with app.app_context():
        from app.storage import get_store
        store = get_store()
        store.put("users", "u_br", {"id": "u_br", "email": "b@example.com", "status": "approved"})
        credentials.save_toss("u_br", "cid_1234567890", "csec", 1)
        credentials.save_notifications("u_br", {
            "enabled": True,
            "paper_notify": False,
            "discord_webhook_url": "https://discord.com/api/webhooks/222/tok",
            "events": {"order": True, "fill": True, "error": True, "halt": True, "briefing": True},
        })

        bot = bots.create("u_br", {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "capital": 1_000_000})
        bot["enabled"] = True
        bots.save(bot)

        # Paper order event is muted when paper_notify=False
        bots.log(bot, "order", "[모의] 매수 주문")
        assert len(sent_msgs) == 0

        # Halt event is always delivered even if bot is in paper mode
        bots.log(bot, "halt", "전체 중지 작동", "warn")
        assert len(sent_msgs) == 1
        assert "전체 중지" in sent_msgs[0][1]["content"]

        # Live fill event is delivered
        bot["mode"] = "live"
        bots.save(bot)
        bots.log(bot, "fill", "매수 체결 10주 @ 70,000")
        assert len(sent_msgs) == 2

        # Scheduler triggers daily close briefing once after session end (+5m) and dedupes on subsequent ticks
        close_dt = datetime(2026, 11, 20, 6, 30, tzinfo=timezone.utc)
        monkeypatch.setattr(
            scheduler,
            "sessions",
            lambda m: [("2026-11-20", close_dt - timedelta(hours=6, minutes=30), close_dt)],
        )
        scheduler.tick(app, now=close_dt + timedelta(minutes=5))
        assert len(sent_msgs) == 3
        assert "일일 브리핑" in sent_msgs[2][1]["content"]

        # Second tick in the same window does not duplicate the briefing
        scheduler.tick(app, now=close_dt + timedelta(minutes=10))
        assert len(sent_msgs) == 3
