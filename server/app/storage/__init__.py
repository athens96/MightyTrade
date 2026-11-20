"""Storage layer.

All persistence goes through the `Store` interface so the backend can be swapped
(JSON file DB now, Supabase later) without touching business code.
"""
from __future__ import annotations

from flask import Flask, current_app

from .base import Store


def init_storage(app: Flask) -> None:
    backend = app.config["STORAGE_BACKEND"]
    if backend == "json":
        from .json_store import JsonFileStore
        store: Store = JsonFileStore(app.config["DATA_DIR"])
    elif backend == "sqlite":
        from .sqlite_store import SqliteStore
        store = SqliteStore(app.config["DATA_DIR"])
    elif backend == "supabase":
        raise NotImplementedError("Supabase store is not implemented yet (see storage/base.py)")
    else:
        raise ValueError(f"Unknown STORAGE_BACKEND: {backend}")
    app.extensions["store"] = store


def get_store() -> Store:
    return current_app.extensions["store"]
