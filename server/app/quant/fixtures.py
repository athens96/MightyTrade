"""Generate shared/fixtures/indicators.json: inputs + Python outputs, asserted equal by the TS tests.

Regenerate after changing indicators.py:  .venv/bin/python -m app.quant.fixtures
"""
from __future__ import annotations

import json
import math
from pathlib import Path

from . import indicators as ind

FIXTURE = Path(__file__).resolve().parents[3] / "shared" / "fixtures" / "indicators.json"


def build() -> dict:
    n = 120
    close = [round(100 + 8 * math.sin(i / 7) + 3 * math.cos(i / 3) + 0.1 * i, 4) for i in range(n)]
    high = [round(c + 1 + abs(math.sin(i)) * 1.5, 4) for i, c in enumerate(close)]
    low = [round(c - 1 - abs(math.cos(i)) * 1.5, 4) for i, c in enumerate(close)]
    bb = ind.bollinger(close, 20, 2)
    dc = ind.donchian(high, low, 20, 10)
    return {
        "input": {"close": close, "high": high, "low": low},
        "output": {
            "sma20": ind.sma(close, 20),
            "ema12": ind.ema(close, 12),
            "rsi14": ind.rsi(close, 14),
            "rsi2": ind.rsi(close, 2),
            "atr14": ind.atr(high, low, close, 14),
            "bb_upper": bb["upper"],
            "bb_lower": bb["lower"],
            "dc_upper": dc["upper"],
            "dc_lower": dc["lower"],
        },
    }


if __name__ == "__main__":
    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_text(json.dumps(build(), indent=1) + "\n")
    print(f"wrote {FIXTURE}")
