from .base import Param, Strategy, StrategyParamError
from .bollinger import BollingerReversion
from .gap_reversion import OpeningGapReversion
from .ibs_reversion import IbsMeanReversion
from .ma_cross import MovingAverageCross
from .macd_trend import MacdTrendSwing
from .orb_breakout import OpeningRangeBreakout
from .rsi2 import Rsi2MeanReversion
from .ts_momentum import TimeSeriesMomentum
from .turtle import Turtle
from .vol_breakout import VolatilityBreakout
from .vwap_reversion import VwapBandReversion

STRATEGIES: dict = {
    cls.id: cls
    for cls in (
        # 스윙 (Swing)
        MovingAverageCross,
        Turtle,
        MacdTrendSwing,
        BollingerReversion,
        Rsi2MeanReversion,
        IbsMeanReversion,
        TimeSeriesMomentum,
        # 단타 (Daytrade)
        VolatilityBreakout,
        OpeningGapReversion,
        OpeningRangeBreakout,
        VwapBandReversion,
    )
}

__all__ = ["STRATEGIES", "Param", "Strategy", "StrategyParamError"]
