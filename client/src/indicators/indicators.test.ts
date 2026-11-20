import { describe, expect, it } from 'vitest'
import * as calc from './calc'
import type { Bars } from './calc'
import { ema, rma, sma } from './math'
import { DEFAULT_INDICATORS, INDICATORS, defaultParams, sanitize } from './registry'

const bars = (close: number[], spread = 1, volume = 100): Bars => ({
  time: close.map((_, i) => 1_700_000_000 + i * 86400),
  open: close.map((c, i) => (i === 0 ? c : close[i - 1])),
  high: close.map((c) => c + spread),
  low: close.map((c) => c - spread),
  close,
  volume: close.map(() => volume),
})
const range = (n: number, f = (i: number) => i + 1) => Array.from({ length: n }, (_, i) => f(i))
const round = (v: number | null, d = 2) => (v === null ? null : Number(v.toFixed(d)))

describe('moving averages', () => {
  it('sma', () => {
    expect(sma([1, 2, 3, 4, 5], 3)).toEqual([null, null, 2, 3, 4])
    expect(sma([null, 1, 2, 3], 2)).toEqual([null, null, 1.5, 2.5])
  })
  it('ema is seeded with the SMA', () => {
    const e = ema([1, 2, 3, 4, 5], 3) // alpha 0.5, seed 2
    expect(e).toEqual([null, null, 2, 3, 4])
  })
  it('rma (Wilder) is seeded with the SMA', () => {
    const r = rma([2, 4, 6, 8], 2) // seed 3, then (3*1+6)/2=4.5, (4.5+8)/2=6.25
    expect(r).toEqual([null, 3, 4.5, 6.25])
  })
})

describe('RSI (Wilder) — StockCharts reference example', () => {
  const closes = [
    44.3389, 44.0902, 44.1497, 43.6124, 44.3278, 44.8264, 45.0955, 45.4245, 45.8433, 46.0826, 45.8931, 46.0328,
    45.614, 46.282, 46.282, 46.0028, 46.0328, 46.4116, 46.2222, 45.6439, 46.2122,
  ]
  it('matches published values', () => {
    const r = calc.rsi(closes, 14)
    expect(r.slice(0, 14).every((v) => v === null)).toBe(true)
    const expected = [70.53, 66.32, 66.55, 69.41, 66.36, 57.97, 62.93]
    r.slice(14).forEach((v, i) => expect(v).toBeCloseTo(expected[i], 1))
  })
  it('is 100 for a strictly rising series', () => {
    expect(calc.rsi(range(30), 14).at(-1)).toBe(100)
  })
})

describe('bands', () => {
  it('bollinger collapses on a flat series and mid = SMA', () => {
    const r = calc.bollinger(range(30, () => 10), 20, 2)
    expect([r.upper.at(-1), r.mid.at(-1), r.lower.at(-1)]).toEqual([10, 10, 10])
    const r2 = calc.bollinger([1, 2, 3, 4], 4, 1)
    expect(r2.mid[3]).toBe(2.5)
    expect(round(r2.upper[3], 4)).toBe(round(2.5 + Math.sqrt(1.25), 4)) // population stdev
  })
  it('envelope', () => {
    const r = calc.envelope(range(5, () => 100), 3, 10)
    expect(r.upper[4]).toBeCloseTo(110, 10)
    expect(r.lower[4]).toBeCloseTo(90, 10)
  })
})

describe('oscillators', () => {
  it('macd is zero on a flat series and hist = macd - signal', () => {
    const flat = calc.macd(range(60, () => 5), 12, 26, 9)
    expect(flat.macd.at(-1)).toBe(0)
    const r = calc.macd(range(60, (i) => Math.sin(i / 3) * 10 + 50), 12, 26, 9)
    expect(r.macd[24]).toBeNull()
    expect(r.signal[25 + 7]).toBeNull()
    expect(r.signal[25 + 8]).not.toBeNull()
    expect(r.hist.at(-1)).toBeCloseTo(r.macd.at(-1)! - r.signal.at(-1)!, 10)
  })
  it('stochastic slow / williams %R at the top of the range', () => {
    const b = bars(range(30), 0)
    const s = calc.stochasticSlow(b, 14, 3, 3)
    expect(s.k[14 + 2 - 1]).toBe(100)
    expect(s.d.at(-1)).toBe(100)
    expect(calc.williamsR(b, 14).at(-1)).toBe(-0)
  })
  it('dmi: uptrend has +DI > -DI and ADX starts after n + adxN - 1 bars', () => {
    const r = calc.dmi(bars(range(60)), 14, 14)
    expect(r.adx[26]).toBeNull()
    expect(r.adx[27]).not.toBeNull()
    expect(r.plusDI.at(-1)!).toBeGreaterThan(r.minusDI.at(-1)!)
  })
  it('cci is 0 on a flat series', () => {
    expect(calc.cci(bars(range(30, () => 7)), 20).at(-1)).toBe(0)
  })
  it('atr equals the constant bar range', () => {
    expect(calc.atr(bars(range(30, () => 7), 2), 14).at(-1)).toBeCloseTo(4, 10)
  })
})

describe('volume', () => {
  it('obv', () => {
    expect(calc.obv(bars([1, 2, 2, 1, 3], 1, 10))).toEqual([0, 10, 10, 0, 10])
  })
  it('mfi is 100 when typical price only rises', () => {
    expect(calc.mfi(bars(range(20)), 14).at(-1)).toBe(100)
  })
  it('vwap resets each session', () => {
    const b: Bars = { time: [0, 60, 86400], open: [10, 10, 30], high: [10, 20, 30], low: [10, 20, 30], close: [10, 20, 30], volume: [1, 1, 5] }
    const v = calc.vwap(b, (t) => String(Math.floor(t / 86400)))
    expect(v).toEqual([10, 15, 30])
  })
})

describe('trend', () => {
  it('parabolic SAR stays below price in an uptrend', () => {
    const b = bars(range(40, (i) => 100 + i * 2))
    const sar = calc.parabolicSar(b, 0.02, 0.2)
    sar.slice(1).forEach((v, i) => expect(v!).toBeLessThan(b.low[i + 1]))
  })
  it('parabolic SAR flips above price after a reversal', () => {
    const b = bars([...range(20, (i) => 100 + i * 2), ...range(20, (i) => 136 - i * 3)])
    const sar = calc.parabolicSar(b, 0.02, 0.2)
    expect(sar.at(-1)!).toBeGreaterThan(b.high.at(-1)!)
  })
  it('ichimoku lines', () => {
    const r = calc.ichimoku(bars(range(60), 0), 9, 26, 52)
    expect(r.tenkan[7]).toBeNull()
    expect(r.tenkan[8]).toBe(5) // (9 + 1) / 2
    expect(r.spanA[25]).toBe((r.tenkan[25]! + r.kijun[25]!) / 2)
    expect(r.spanB[51]).toBe(26.5)
  })
  it('disparity / psychological line', () => {
    expect(calc.disparity(range(10, () => 50), 5).at(-1)).toBe(100)
    expect(calc.psychologicalLine([1, 2, 1, 2, 1, 2, 1], 4).at(-1)).toBe(50)
  })
})

describe('registry', () => {
  const noisy = range(300, (i) => 100 + Math.sin(i / 7) * 8 + Math.cos(i / 3) * 3 + i * 0.1)
  const b = bars(noisy, 1.5, 1000)
  it.each(INDICATORS.map((d) => [d.id, d] as const))('%s computes aligned plots with defaults', (_, def) => {
    const out = def.compute(b, defaultParams(def), { market: 'KR', intraday: true })
    for (const plot of out.plots) {
      expect(plot.values).toHaveLength(b.close.length)
      expect(plot.values.some((v) => v !== null && Number.isFinite(v))).toBe(true)
    }
  })
  it('sanitize drops unknown ids, duplicates and invalid params', () => {
    const out = sanitize([{ id: 'nope' }, { id: 'rsi', params: { n: -3 } }, { id: 'rsi', params: { n: 9 } }])
    expect(out).toEqual([{ id: 'rsi', params: { n: 14 } }])
    expect(sanitize(null)).toEqual(DEFAULT_INDICATORS)
  })
})
