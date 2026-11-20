from __future__ import annotations

from ..models import Order, OrderRequest, Position
from .base import Broker


class TossBroker(Broker):
    """TODO: Toss Securities order API. Per-user keys are stored encrypted (app/crypto.py)."""

    name = "toss"

    def place_order(self, req: OrderRequest) -> str:
        raise NotImplementedError("Toss order is not implemented yet")

    def cancel_order(self, broker_order_id: str) -> None:
        raise NotImplementedError("Toss cancel is not implemented yet")

    def get_order(self, broker_order_id: str) -> Order:
        raise NotImplementedError

    def get_positions(self) -> list[Position]:
        raise NotImplementedError("Toss positions are not implemented yet")

    def get_balance(self) -> dict:
        raise NotImplementedError("Toss balance is not implemented yet")
