from .base import Param, Strategy, StrategyParamError
from .bollinger import BollingerReversion
from .ma_cross import MovingAverageCross
from .rsi2 import Rsi2MeanReversion
from .turtle import Turtle
from .vol_breakout import VolatilityBreakout

STRATEGIES: dict = {
    cls.id: cls for cls in (MovingAverageCross, VolatilityBreakout, Rsi2MeanReversion, BollingerReversion, Turtle)
}

__all__ = ["STRATEGIES", "Param", "Strategy", "StrategyParamError"]
