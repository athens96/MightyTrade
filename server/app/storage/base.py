from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any, Callable, ContextManager, Optional

Record = dict[str, Any]


class Store(ABC):
    """Minimal document store: collection -> id -> record.

    Maps 1:1 to a table per collection in Supabase/Postgres later.
    Collections in use: users, watchlists, settings, orders, strategies.
    """

    @abstractmethod
    def get(self, collection: str, id: str) -> Optional[Record]: ...

    @abstractmethod
    def put(self, collection: str, id: str, record: Record) -> Record: ...

    @abstractmethod
    def delete(self, collection: str, id: str) -> bool: ...

    @abstractmethod
    def find(self, collection: str, predicate: Callable[[Record], bool] | None = None) -> list[Record]: ...

    @abstractmethod
    def lock(self, name: str) -> ContextManager[None]:
        """Exclusive lock shared by every process using this store (e.g. token refresh)."""

    def find_one(self, collection: str, predicate: Callable[[Record], bool]) -> Optional[Record]:
        rows = self.find(collection, predicate)
        return rows[0] if rows else None
