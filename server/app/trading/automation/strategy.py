"""Auto-trading building blocks (not wired yet).

Planned flow:
  scheduler (APScheduler or a separate worker process)
    -> for each enabled user strategy: fetch candles via market registry
    -> Strategy.evaluate() -> Signal
    -> risk checks (max position, daily loss limit, market hours)
    -> Broker.place_order()
    -> persist to `orders` collection + notify

Run the scheduler as a separate process from gunicorn so multiple web workers
don't fire the same strategy twice.
"""
from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Optional

from ...market.models import Candle
from ..models import Side


@dataclass
class Signal:
    side: Side
    quantity: float
    price: Optional[float] = None
    reason: str = ""


class Strategy(ABC):
    name = "base"

    def __init__(self, params: dict):
        self.params = params

    @abstractmethod
    def evaluate(self, candles: list[Candle]) -> Optional[Signal]:
        """Return a Signal to trade, or None to do nothing."""
