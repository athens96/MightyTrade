"""Tests for SqliteStore (WAL mode), thread safety, JSON migration, and Flask integration."""
import threading

import pytest

from app import create_app
from app.storage.json_store import JsonFileStore
from app.storage.sqlite_store import SqliteStore, migrate_json_to_sqlite


def test_sqlite_store_crud_wal_and_isolation(tmp_path):
    store = SqliteStore(tmp_path)
    mode = store._conn().execute("PRAGMA journal_mode").fetchone()[0]
    assert mode.lower() == "wal"

    saved = store.put("users", "u1", {"name": "Alice", "nested": {"a": 1}})
    assert saved["id"] == "u1" and saved["name"] == "Alice"
    saved["nested"]["a"] = 999  # mutating returned dict must not affect DB
    assert store.get("users", "u1")["nested"]["a"] == 1

    store.put("users", "u2", {"name": "Bob", "active": True})
    store.put("orders", "u1", {"symbol": "005930"})  # same id in different collection
    assert len(store.find("users")) == 2
    assert store.find_one("users", lambda r: r.get("active"))["id"] == "u2"
    assert store.get("orders", "u1")["symbol"] == "005930"

    assert store.delete("users", "u1") is True
    assert store.delete("users", "u1") is False
    assert store.get("users", "u1") is None
    with pytest.raises(ValueError):
        store.get("../escape", "x")
    store.close()


def test_sqlite_store_concurrent_writes_and_named_lock(tmp_path):
    store = SqliteStore(tmp_path)
    errors = []
    shared = {"counter": 0}

    def worker(tid):
        try:
            for i in range(15):
                store.put("bot_events", f"t{tid}_{i}", {"tid": tid, "i": i})
                with store.lock("token_refresh"):
                    val = shared["counter"]
                    shared["counter"] = val + 1
        except Exception as e:  # noqa: BLE001
            errors.append(e)
        finally:
            store.close()

    threads = [threading.Thread(target=worker, args=(k,)) for k in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors
    assert shared["counter"] == 60
    assert len(store.find("bot_events")) == 60
    store.close()


def test_migrate_json_to_sqlite_is_idempotent(tmp_path):
    js = JsonFileStore(tmp_path)
    js.put("users", "u1", {"email": "a@example.com"})
    js.put("bots", "b1", {"user_id": "u1", "symbol": "005930"})

    first = migrate_json_to_sqlite(tmp_path)
    assert first == {"bots": 1, "users": 1}
    # Second run skips existing records unless overwrite=True
    second = migrate_json_to_sqlite(tmp_path)
    assert second == {"bots": 0, "users": 0}

    sq = SqliteStore(tmp_path)
    assert sq.get("users", "u1")["email"] == "a@example.com"
    assert sq.get("bots", "b1")["symbol"] == "005930"
    sq.close()


def test_app_boots_with_sqlite_backend(tmp_path):
    app = create_app("testing", {
        "STORAGE_BACKEND": "sqlite",
        "DATA_DIR": tmp_path,
        "GOOGLE_CLIENT_ID": "test",
        "ADMIN_EMAILS": ["admin@example.com"],
    })
    assert isinstance(app.extensions["store"], SqliteStore)
    client = app.test_client()
    assert client.get("/api/health").json == {"status": "ok"}
    app.extensions["store"].close()
