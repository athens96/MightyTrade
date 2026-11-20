"""Auto-trading (not wired yet).

Strategies live in app/quant/strategies and are shared with the backtester:
    strategy.prepare(bars); orders = strategy.on_bar(len(bars) - 1, ctx)

Planned live flow:
  scheduler (separate process from gunicorn, so web workers don't fire twice)
    -> per enabled user strategy: load latest candles with that user's Toss key
    -> prepare() + on_bar(last bar) -> orders
    -> risk checks (max position, daily loss limit, market hours, dry-run mode)
    -> Broker.place_order() -> persist to `orders` + notify
"""
from ...quant.strategies import STRATEGIES, Strategy

__all__ = ["STRATEGIES", "Strategy"]
