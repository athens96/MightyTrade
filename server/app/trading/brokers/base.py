from __future__ import annotations

from abc import ABC, abstractmethod

from ..models import Order, OrderRequest, Position


class Broker(ABC):
    """Order execution interface. One instance per user (holds that user's credentials)."""

    name = "base"

    def __init__(self, credentials: dict):
        self.credentials = credentials

    @abstractmethod
    def place_order(self, req: OrderRequest) -> str:
        """Submit an order, return the broker-side order id."""

    @abstractmethod
    def cancel_order(self, broker_order_id: str) -> None: ...

    @abstractmethod
    def get_order(self, broker_order_id: str) -> Order: ...

    @abstractmethod
    def get_positions(self) -> list[Position]: ...

    @abstractmethod
    def get_balance(self) -> dict: ...
