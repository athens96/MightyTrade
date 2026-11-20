from __future__ import annotations

import copy
import fcntl
import json
import os
import tempfile
import threading
from contextlib import contextmanager
from pathlib import Path
from typing import Callable, Iterator, Optional

from .base import Record, Store


class JsonFileStore(Store):
    """One JSON file per collection under DATA_DIR.

    Safe across threads (lock) and gunicorn workers (fcntl.flock on a lock file).
    Writes are atomic (tmp file + os.replace). Good enough for a single-node
    personal deployment; replace with Supabase for anything bigger.
    """

    def __init__(self, data_dir: Path):
        self.dir = Path(data_dir)
        self.dir.mkdir(parents=True, exist_ok=True)
        self._lock = threading.RLock()

    def _path(self, collection: str) -> Path:
        if not collection.replace("_", "").isalnum():
            raise ValueError(f"invalid collection name: {collection}")
        return self.dir / f"{collection}.json"

    @contextmanager
    def _locked(self, collection: str) -> Iterator[None]:
        lock_path = self.dir / f".{collection}.lock"
        with self._lock, open(lock_path, "a") as lf:
            fcntl.flock(lf, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(lf, fcntl.LOCK_UN)

    def lock(self, name: str):
        self._path(name)  # validates the name
        return self._locked(f"lock_{name}")

    def _read(self, collection: str) -> dict[str, Record]:
        path = self._path(collection)
        if not path.exists():
            return {}
        with open(path, encoding="utf-8") as f:
            return json.load(f)

    def _write(self, collection: str, data: dict[str, Record]) -> None:
        path = self._path(collection)
        fd, tmp = tempfile.mkstemp(dir=self.dir, prefix=f".{collection}.", suffix=".tmp")
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                json.dump(data, f, ensure_ascii=False, indent=2)
            os.replace(tmp, path)
        except BaseException:
            if os.path.exists(tmp):
                os.unlink(tmp)
            raise

    def get(self, collection: str, id: str) -> Optional[Record]:
        with self._locked(collection):
            rec = self._read(collection).get(id)
        return copy.deepcopy(rec) if rec is not None else None

    def put(self, collection: str, id: str, record: Record) -> Record:
        record = {**record, "id": id}
        with self._locked(collection):
            data = self._read(collection)
            data[id] = record
            self._write(collection, data)
        return copy.deepcopy(record)

    def delete(self, collection: str, id: str) -> bool:
        with self._locked(collection):
            data = self._read(collection)
            existed = data.pop(id, None) is not None
            if existed:
                self._write(collection, data)
        return existed

    def find(self, collection: str, predicate: Callable[[Record], bool] | None = None) -> list[Record]:
        with self._locked(collection):
            rows = list(self._read(collection).values())
        if predicate:
            rows = [r for r in rows if predicate(r)]
        return copy.deepcopy(rows)
