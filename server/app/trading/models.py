from __future__ import annotations

from dataclasses import asdict, dataclass, field
from enum import Enum
from typing import Optional

from ..market.models import Market


class Side(str, Enum):
    BUY = "buy"
    SELL = "sell"


class OrderType(str, Enum):
    MARKET = "market"
    LIMIT = "limit"


class OrderStatus(str, Enum):
    PENDING = "pending"
    SUBMITTED = "submitted"
    FILLED = "filled"
    PARTIALLY_FILLED = "partially_filled"
    CANCELED = "canceled"
    REJECTED = "rejected"


@dataclass
class OrderRequest:
    market: Market
    symbol: str
    side: Side
    type: OrderType
    quantity: float
    price: Optional[float] = None  # required for LIMIT


@dataclass
class Order:
    id: str
    user_id: str
    request: OrderRequest
    status: OrderStatus = OrderStatus.PENDING
    broker_order_id: Optional[str] = None
    filled_quantity: float = 0
    created_at: str = ""
    meta: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass
class Position:
    market: Market
    symbol: str
    quantity: float
    avg_price: float
