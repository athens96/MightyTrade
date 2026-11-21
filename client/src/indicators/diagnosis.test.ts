import { describe, expect, it } from 'vitest'
import type { Candle } from '../api/types'
import { diagnoseChart } from './diagnosis'

function makeCandles(
  count: number,
  priceFn: (i: number) => number,
  stepSec = 86400,
  volFn: (i: number) => number = () => 100_000,
): Candle[] {
  const start = 1_700_000_000
  const out: Candle[] = []
  for (let i = 0; i < count; i++) {
    const c = priceFn(i)
    const prev = i > 0 ? out[i - 1].close : c
    const o = (prev + c) / 2
    const hi = Math.max(o, c) * 1.008
    const lo = Math.min(o, c) * 0.992
    out.push({ time: start + i * stepSec, open: o, high: hi, low: lo, close: c, volume: volFn(i) })
  }
  return out
}

describe('diagnoseChart', () => {
  it('returns null when fewer than 25 candles are provided', () => {
    const short = makeCandles(20, (i) => 100 + i)
    expect(diagnoseChart(short, 'KR', '1d')).toBeNull()
  })

  it('identifies a steady uptrend with positive score and bullish signals', () => {
    // Gentle ~0.4%/bar climb with mild oscillation so RSI stays moderate-warm while MAs align
    const up = makeCandles(
      90,
      (i) => 50_000 * Math.pow(1.004, i) * (1 + 0.008 * Math.sin(i * 0.9)),
      86400,
      (i) => (i === 89 ? 250_000 : 100_000),
    )
    const res = diagnoseChart(up, 'KR', '1d')
    expect(res).not.toBeNull()
    expect(res!.score).toBeGreaterThanOrEqual(63)
    expect(['bull', 'overheated']).toContain(res!.overallTone)
    expect(res!.signals.some((s) => s.id === 'ma' && s.tone === 'bull')).toBe(true)
    expect(res!.signals.some((s) => s.id === 'volume' && s.tone === 'bull')).toBe(true)
    expect(res!.support).not.toBeNull()
    expect(res!.atr).not.toBeNull()
  })

  it('identifies a sustained downtrend with low score and bearish caution', () => {
    const down = makeCandles(90, (i) => 80_000 * Math.pow(0.994, i) * (1 + 0.005 * Math.cos(i)))
    const res = diagnoseChart(down, 'KR', '1d')
    expect(res).not.toBeNull()
    expect(res!.score).toBeLessThan(44)
    expect(['bear', 'oversold_bear']).toContain(res!.overallTone)
    expect(res!.signals.some((s) => s.id === 'ma' && s.tone === 'bear')).toBe(true)
  })

  it('detects short-term oversold pullback after an uptrend', () => {
    // 80 days of strong climb followed by 5 sharp drop days
    const pullback = makeCandles(85, (i) => (i < 80 ? 100 * Math.pow(1.006, i) : 100 * Math.pow(1.006, 79) * Math.pow(0.965, i - 79)))
    const res = diagnoseChart(pullback, 'US', '1d')
    expect(res).not.toBeNull()
    expect(res!.signals.some((s) => (s.id === 'rsi' || s.id === 'bb') && s.tone === 'cool')).toBe(true)
  })

  it('includes intraday VWAP signal on 5m candles and reflects livePrice override', () => {
    const intra = makeCandles(60, (i) => 200 + i * 0.2, 300)
    const res = diagnoseChart(intra, 'US', '5m', 250)
    expect(res).not.toBeNull()
    expect(res!.intervalLabel).toContain('5분봉')
    expect(res!.signals.some((s) => s.id === 'vwap' && s.tone === 'bull')).toBe(true)
  })
})
