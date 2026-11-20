"""Strategy interface shared by the backtester, paper trading and live auto-trading.

    prepare(bars)          compute indicators once over all bars
    on_bar(i, ctx)         decide at the CLOSE of bar i -> orders that may fill during bar i+1

Live trading will call prepare() on the latest candles and on_bar() for the last bar, so a strategy
behaves the same way in a backtest and in production.
"""
from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import asdict, dataclass
from typing import Any, ClassVar, Optional

from ..engine import Bars, Context, Order


@dataclass(frozen=True)
class Param:
    key: str
    label: str
    default: float
    min: float
    max: float
    step: float = 1
    integer: bool = True


class StrategyParamError(ValueError):
    pass


class Strategy(ABC):
    id: ClassVar[str]
    name: ClassVar[str]
    summary: ClassVar[str]  # one line
    description: ClassVar[str]  # rules + how to read results
    params: ClassVar[list]  # list[Param]

    def __init__(self, raw: Optional[dict] = None):
        self.p = self.resolve(raw or {})
        self.validate()

    @classmethod
    def resolve(cls, raw: dict) -> dict[str, Any]:
        out: dict[str, Any] = {}
        for prm in cls.params:
            v = raw.get(prm.key, prm.default)
            try:
                v = float(v)
            except (TypeError, ValueError):
                raise StrategyParamError(f"{prm.label}: 숫자를 입력하세요")
            if not prm.min <= v <= prm.max:
                raise StrategyParamError(f"{prm.label}: {prm.min:g} ~ {prm.max:g} 범위여야 합니다")
            out[prm.key] = int(v) if prm.integer else v
        return out

    def validate(self) -> None:
        """Cross-parameter checks. Raise StrategyParamError with a user-facing message."""

    @abstractmethod
    def warmup(self) -> int:
        """Bars needed before the first valid signal."""

    @abstractmethod
    def prepare(self, bars: Bars) -> None: ...

    @abstractmethod
    def on_bar(self, i: int, ctx: Context) -> list: ...  # list[Order]

    def chart_indicators(self) -> list:
        """Client indicator configs ({id, params}, see client/src/indicators/registry.ts) to draw with results."""
        return []

    @classmethod
    def meta(cls) -> dict[str, Any]:
        return {
            "id": cls.id,
            "name": cls.name,
            "summary": cls.summary,
            "description": cls.description,
            "params": [asdict(p) for p in cls.params],
        }


__all__ = ["Bars", "Context", "Order", "Param", "Strategy", "StrategyParamError"]
