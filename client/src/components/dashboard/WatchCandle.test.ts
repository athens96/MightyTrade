import { describe, expect, it } from 'vitest'
import { computeWatchCandle } from './WatchCandle'

describe('computeWatchCandle', () => {
  it('computes bullish candle geometry with wicks and prevClose baseline', () => {
    const g = computeWatchCandle({
      symbol: '005930',
      price: 72000,
      open: 70500,
      high: 72500,
      low: 70000,
      prev_close: 70200,
      change: 1800,
      change_pct: 2.56,
    })
    expect(g).not.toBeNull()
    expect(g!.kind).toBe('bull')
    expect(g!.color).toBe('#e5484d')
    expect(g!.wickTop).toBeLessThan(g!.bodyTop)
    expect(g!.wickBottom).toBeGreaterThan(g!.bodyTop + g!.bodyHeight - 0.1)
    expect(g!.prevCloseY).not.toBeNull()
    expect(g!.intradayPosPct).toBe(80)
  })

  it('distinguishes a gap-up bearish candle (close < open but above prev_close)', () => {
    const g = computeWatchCandle({
      symbol: '000660',
      price: 205000,
      open: 210000,
      high: 212000,
      low: 204000,
      prev_close: 200000,
      change: 5000,
      change_pct: 2.5,
    })
    expect(g).not.toBeNull()
    expect(g!.kind).toBe('bear')
    expect(g!.color).toBe('#3e63dd')
    // prev_close is below low, so prevCloseY should sit below wickBottom
    expect(g!.prevCloseY!).toBeGreaterThan(g!.wickBottom)
  })

  it('prevents tiny price fluctuations from stretching into a full-height body', () => {
    const g = computeWatchCandle({
      symbol: 'AAPL',
      price: 200.05,
      open: 200.0,
      high: 200.05,
      low: 200.0,
      prev_close: 200.0,
      change: 0.05,
      change_pct: 0.03,
    })
    expect(g).not.toBeNull()
    expect(g!.kind).toBe('bull')
    // With 1% minimum span (2.0), a 0.05 move stays compact (min 2px body)
    expect(g!.bodyHeight).toBe(2)
  })

  it('returns null for missing or errored quotes', () => {
    expect(computeWatchCandle(undefined)).toBeNull()
    expect(computeWatchCandle({ symbol: 'ERR', error: 'not_found' })).toBeNull()
  })
})
