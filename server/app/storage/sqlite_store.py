"""SQLite (WAL mode) Store implementation — drop-in replacement for JsonFileStore.

Optimized for single-VM deployments: indexed O(1) point lookups/writes, atomic transactions,
concurrent readers during writes (WAL), and no full-file JSON rewrite on every mutation.
Cross-process named locks (`lock(name)`) still use fcntl.flock so token refresh / key init
stay coordinated even across separate processes.
"""
from __future__ import annotations

import copy
import fcntl
import json
import sqlite3
import threading
from contextlib import contextmanager
from pathlib import Path
from typing import Callable, Iterator, Optional

from .base import Record, Store

SCHEMA = """
CREATE TABLE IF NOT EXISTS records (
    collection TEXT NOT NULL,
    id         TEXT NOT NULL,
    data       TEXT NOT NULL,
    PRIMARY KEY (collection, id)
);
CREATE INDEX IF NOT EXISTS idx_records_collection ON records(collection);
"""


def _validate(name: str) -> None:
    if not name or not name.replace("_", "").isalnum():
        raise ValueError(f"invalid name: {name}")


class SqliteStore(Store):
    def __init__(self, data_dir: Path, filename: str = "mighty_trade.db"):
        self.dir = Path(data_dir)
        self.dir.mkdir(parents=True, exist_ok=True)
        self.db_path = self.dir / filename
        self._local = threading.local()
        self._write_lock = threading.RLock()
        self._named_lock = threading.RLock()
        # Initialize schema + WAL mode once on startup.
        conn = self._conn()
        with conn:
            conn.executescript(SCHEMA)

    def _conn(self) -> sqlite3.Connection:
        conn: Optional[sqlite3.Connection] = getattr(self._local, "conn", None)
        if conn is None:
            conn = sqlite3.connect(str(self.db_path), timeout=15.0, isolation_level=None)
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("PRAGMA synchronous=NORMAL")
            conn.execute("PRAGMA busy_timeout=10000")
            self._local.conn = conn
        return conn

    def close(self) -> None:
        conn: Optional[sqlite3.Connection] = getattr(self._local, "conn", None)
        if conn is not None:
            conn.close()
            self._local.conn = None

    @contextmanager
    def lock(self, name: str) -> Iterator[None]:
        _validate(name)
        lock_path = self.dir / f".lock_{name}.lock"
        with self._named_lock, open(lock_path, "a") as lf:
            fcntl.flock(lf, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(lf, fcntl.LOCK_UN)

    def get(self, collection: str, id: str) -> Optional[Record]:
        _validate(collection)
        row = self._conn().execute(
            "SELECT data FROM records WHERE collection = ? AND id = ?",
            (collection, id),
        ).fetchone()
        return json.loads(row[0]) if row else None

    def put(self, collection: str, id: str, record: Record) -> Record:
        _validate(collection)
        stored = {**record, "id": id}
        raw = json.dumps(stored, ensure_ascii=False)
        with self._write_lock:
            conn = self._conn()
            conn.execute("BEGIN IMMEDIATE")
            try:
                conn.execute(
                    "INSERT OR REPLACE INTO records(collection, id, data) VALUES (?, ?, ?)",
                    (collection, id, raw),
                )
                conn.execute("COMMIT")
            except BaseException:
                conn.execute("ROLLBACK")
                raise
        return copy.deepcopy(stored)

    def delete(self, collection: str, id: str) -> bool:
        _validate(collection)
        with self._write_lock:
            conn = self._conn()
            conn.execute("BEGIN IMMEDIATE")
            try:
                cur = conn.execute(
                    "DELETE FROM records WHERE collection = ? AND id = ?",
                    (collection, id),
                )
                conn.execute("COMMIT")
                return cur.rowcount > 0
            except BaseException:
                conn.execute("ROLLBACK")
                raise

    def find(self, collection: str, predicate: Callable[[Record], bool] | None = None) -> list[Record]:
        _validate(collection)
        cur = self._conn().execute(
            "SELECT data FROM records WHERE collection = ?",
            (collection,),
        )
        rows = [json.loads(r[0]) for r in cur.fetchall()]
        if predicate:
            rows = [r for r in rows if predicate(r)]
        return rows


def migrate_json_to_sqlite(data_dir: Path, filename: str = "mighty_trade.db", overwrite: bool = False) -> dict[str, int]:
    """Import every collection `*.json` in `data_dir` into SQLite. Returns {collection: imported_count}."""
    data_dir = Path(data_dir)
    store = SqliteStore(data_dir, filename=filename)
    counts: dict[str, int] = {}
    for path in sorted(data_dir.glob("*.json")):
        coll = path.stem
        try:
            _validate(coll)
            with open(path, encoding="utf-8") as f:
                data = json.load(f)
        except (ValueError, OSError):
            continue
        if not isinstance(data, dict):
            continue
        imported = 0
        for rec_id, rec in data.items():
            if not isinstance(rec, dict):
                continue
            if not overwrite and store.get(coll, str(rec_id)) is not None:
                continue
            store.put(coll, str(rec_id), rec)
            imported += 1
        counts[coll] = imported
    store.close()
    return counts


if __name__ == "__main__":  # pragma: no cover
    import sys
    target = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("./data")
    result = migrate_json_to_sqlite(target)
    print(f"Migrated JSON collections in {target} -> SQLite:", result)
