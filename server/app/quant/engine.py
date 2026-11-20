"""Bar-by-bar backtest engine (long-only, one position at a time).

Timing rule (prevents look-ahead bias): a strategy decides at the CLOSE of bar i using data up to i;
its orders can only fill during bar i+1:
- market: at bar i+1 open
- stop:   if bar i+1 trades through the level; fill at the level, or at the open if it gapped past it
Costs: commission on both sides, sell tax (e.g. KR securities transaction tax), slippage on every fill.
"""
from __future__ import annotations

import math
from dataclasses import asdict, dataclass, field
from typing import TYPE_CHECKING, Any, Optional

from .metrics import drawdown_series, performance, trade_stats

if TYPE_CHECKING:
    from .strategies.base import Strategy


@dataclass
class Bars:
    time: list
    open: list
    high: list
    low: list
    close: list
    volume: list

    @classmethod
    def from_candles(cls, candles: list) -> "Bars":
        return cls(
            time=[c.time for c in candles],
            open=[c.open for c in candles],
            high=[c.high for c in candles],
            low=[c.low for c in candles],
            close=[c.close for c in candles],
            volume=[c.volume for c in candles],
        )

    def __len__(self) -> int:
        return len(self.close)


@dataclass
class Order:
    side: str  # "buy" | "sell"
    kind: str = "market"  # "market" | "stop"
    stop: Optional[float] = None  # absolute stop level
    stop_from_open: Optional[float] = None  # level = next bar's open + this (volatility breakout)
    qty: Optional[float] = None  # buy: shares (None = use all cash). sell: always closes the position
    reason: str = ""


@dataclass
class Position:
    qty: float = 0.0
    entry_price: float = 0.0
    entry_index: int = -1
    entry_atr: Optional[float] = None

    @property
    def is_open(self) -> bool:
        return self.qty > 0


@dataclass
class Context:
    """Read-only state handed to Strategy.on_bar at the close of a bar."""

    position: Position
    cash: float
    equity: float


@dataclass
class Costs:
    commission_pct: float = 0.015  # per side, % of traded value
    tax_pct: float = 0.0  # on sells, % of traded value
    slippage_pct: float = 0.05  # per fill, % of price (adverse)


@dataclass
class Trade:
    entry_time: int
    entry_price: float
    qty: float
    entry_reason: str
    exit_time: Optional[int] = None
    exit_price: Optional[float] = None
    exit_reason: str = ""
    pnl: Optional[float] = None  # net of all costs
    return_pct: Optional[float] = None
    bars: int = 0
    open: bool = True
    _cost_basis: float = field(default=0.0, repr=False)  # entry value + entry commission
    _entry_index: int = field(default=0, repr=False)

    def public(self) -> dict:
        d = asdict(self)
        d.pop("_cost_basis")
        d.pop("_entry_index")
        return d


class Backtester:
    def __init__(self, bars: Bars, strategy: "Strategy", capital: float, costs: Costs, start_index: int,
                 whole_shares: bool = True):
        if not 0 <= start_index < len(bars) - 1:
            raise ValueError("not enough bars for the backtest period")
        self.b = bars
        self.strategy = strategy
        self.capital = capital
        self.costs = costs
        self.start = start_index
        self.whole_shares = whole_shares
        self.cash = capital
        self.pos = Position()
        self.trades: list[Trade] = []
        self.warnings: list[str] = []

    # ------------------------------------------------------------------ fills
    def _fill_price(self, o: Order, j: int) -> Optional[float]:
        b, slip = self.b, self.costs.slippage_pct / 100
        if o.kind == "market":
            raw = b.open[j]
        elif o.kind == "stop":
            level = o.stop if o.stop is not None else b.open[j] + (o.stop_from_open or 0.0)
            if o.side == "buy":
                if b.high[j] < level:
                    return None
                raw = max(b.open[j], level)
            else:
                if b.low[j] > level:
                    return None
                raw = min(b.open[j], level)
        else:
            raise ValueError(f"unknown order kind {o.kind}")
        return raw * (1 + slip) if o.side == "buy" else raw * (1 - slip)

    def _buy(self, o: Order, j: int) -> None:
        if self.pos.is_open:
            return  # one position at a time (no pyramiding)
        price = self._fill_price(o, j)
        if price is None:
            return
        comm = self.costs.commission_pct / 100
        affordable = self.cash / (price * (1 + comm))
        qty = min(o.qty, affordable) if o.qty is not None else affordable
        if self.whole_shares:
            qty = math.floor(qty)
        if qty <= 0:
            if "insufficient_capital" not in self.warnings:
                self.warnings.append("insufficient_capital")
            return
        value = qty * price
        fee = value * comm
        self.cash -= value + fee
        self.pos = Position(qty, price, j)
        self.trades.append(Trade(self.b.time[j], price, qty, o.reason, _cost_basis=value + fee, _entry_index=j))

    def _sell(self, o: Order, j: int) -> None:
        if not self.pos.is_open:
            return
        price = self._fill_price(o, j)
        if price is None:
            return
        value = self.pos.qty * price
        fee = value * self.costs.commission_pct / 100
        tax = value * self.costs.tax_pct / 100
        self.cash += value - fee - tax
        t = self.trades[-1]
        t.exit_time, t.exit_price, t.exit_reason, t.open = self.b.time[j], price, o.reason, False
        t.pnl = value - fee - tax - t._cost_basis
        t.return_pct = t.pnl / t._cost_basis * 100
        t.bars = j - t._entry_index
        self.pos = Position()

    # -------------------------------------------------------------------- run
    def run(self) -> dict[str, Any]:
        b = self.b
        self.strategy.prepare(b)
        pending: list[Order] = []
        equity: list[float] = []
        in_market = 0
        for j in range(self.start, len(b)):
            for o in pending:  # orders decided at the previous close
                (self._buy if o.side == "buy" else self._sell)(o, j)
            if self.pos.is_open:
                in_market += 1
            eq = self.cash + self.pos.qty * b.close[j]
            equity.append(eq)
            pending = self.strategy.on_bar(j, Context(self.pos, self.cash, eq)) or []

        # mark an open position to the last close for reporting
        if self.pos.is_open:
            t = self.trades[-1]
            last = len(b) - 1
            value = self.pos.qty * b.close[last]
            t.pnl = value - t._cost_basis
            t.return_pct = t.pnl / t._cost_basis * 100
            t.bars = last - t._entry_index

        times = b.time[self.start:]
        bench = self._buy_and_hold()
        trades = [t.public() for t in self.trades]
        return {
            "equity": [
                {"time": t, "equity": round(e, 4), "benchmark": round(be, 4), "drawdown": round(dd * 100, 4)}
                for t, e, be, dd in zip(times, equity, bench, drawdown_series(equity))
            ],
            "metrics": {**performance(times, equity), **trade_stats(trades),
                        "exposure_pct": in_market / len(equity) * 100},
            "benchmark": performance(times, bench),
            "trades": trades,
            "warnings": self.warnings,
        }

    def _buy_and_hold(self) -> list[float]:
        return buy_and_hold(self.b, self.start, len(self.b), self.capital, self.costs, self.whole_shares)


def buy_and_hold(bars: Bars, start: int, end: int, capital: float, costs: Costs, whole_shares: bool = True) -> list:
    """Equity of buying at bars[start] close (same costs) and holding through bars[end - 1]."""
    price = bars.close[start] * (1 + costs.slippage_pct / 100)
    qty = capital / (price * (1 + costs.commission_pct / 100))
    if whole_shares:
        qty = math.floor(qty)
    cash = capital - qty * price * (1 + costs.commission_pct / 100)
    return [cash + qty * bars.close[j] for j in range(start, end)]


def slice_bars(bars: Bars, end: int) -> Bars:
    """Bars [0, end): used to run a segment without any data after it (no look-ahead)."""
    return Bars(bars.time[:end], bars.open[:end], bars.high[:end], bars.low[:end], bars.close[:end], bars.volume[:end])
