// Technical indicator formulas. Pure functions over OHLCV arrays; outputs are index-aligned with input bars.
import { ema, highest, lowest, map, rma, sma, stdev, trueRange, zip, type Series } from './math'

export interface Bars {
  time: number[] // unix seconds (UTC)
  open: number[]
  high: number[]
  low: number[]
  close: number[]
  volume: number[]
}

// ---------------------------------------------------------------- trend / overlays

export function bollinger(close: number[], n: number, k: number) {
  const mid = sma(close, n)
  const sd = stdev(close, n)
  return {
    upper: zip(mid, sd, (m, s) => m + k * s),
    mid,
    lower: zip(mid, sd, (m, s) => m - k * s),
  }
}

/** Donchian / price channel: highest high and lowest low over separate lookbacks. */
export function donchian(b: Bars, upperN: number, lowerN: number) {
  return { upper: highest(b.high, upperN), lower: lowest(b.low, lowerN) }
}

export function envelope(close: number[], n: number, pct: number) {
  const mid = sma(close, n)
  return { upper: map(mid, (m) => m * (1 + pct / 100)), mid, lower: map(mid, (m) => m * (1 - pct / 100)) }
}

/**
 * Ichimoku. Spans are returned unshifted; the chart plots spanA/spanB `displacement - 1` bars ahead
 * and the lagging span `displacement - 1` bars back (the current bar counts as 1, as in KR HTS / TradingView).
 */
export function ichimoku(b: Bars, conv: number, base: number, spanB: number) {
  const mid = (n: number) => zip(highest(b.high, n), lowest(b.low, n), (h, l) => (h + l) / 2)
  const tenkan = mid(conv)
  const kijun = mid(base)
  return { tenkan, kijun, spanA: zip(tenkan, kijun, (t, k) => (t + k) / 2), spanB: mid(spanB), chikou: [...b.close] as Series }
}

/** Parabolic SAR (Wilder). */
export function parabolicSar(b: Bars, step: number, maxStep: number): Series {
  const n = b.close.length
  const out: Series = new Array(n).fill(null)
  if (n < 2) return out
  let up = b.high[1] + b.low[1] >= b.high[0] + b.low[0]
  let sar = up ? b.low[0] : b.high[0]
  let ep = up ? b.high[1] : b.low[1]
  let af = step
  out[1] = sar
  for (let i = 2; i < n; i++) {
    sar = sar + af * (ep - sar)
    if (up) {
      sar = Math.min(sar, b.low[i - 1], b.low[i - 2])
      if (b.low[i] < sar) {
        up = false
        sar = ep
        ep = b.low[i]
        af = step
      } else if (b.high[i] > ep) {
        ep = b.high[i]
        af = Math.min(af + step, maxStep)
      }
    } else {
      sar = Math.max(sar, b.high[i - 1], b.high[i - 2])
      if (b.high[i] > sar) {
        up = true
        sar = ep
        ep = b.high[i]
        af = step
      } else if (b.low[i] < ep) {
        ep = b.low[i]
        af = Math.min(af + step, maxStep)
      }
    }
    out[i] = sar
  }
  return out
}

/** Session VWAP, reset when `sessionKey` changes (trading date in the market's timezone). */
export function vwap(b: Bars, sessionKey: (t: number) => string): Series {
  let key = ''
  let pv = 0
  let vol = 0
  return b.close.map((c, i) => {
    const k = sessionKey(b.time[i])
    if (k !== key) {
      key = k
      pv = 0
      vol = 0
    }
    const tp = (b.high[i] + b.low[i] + c) / 3
    pv += tp * b.volume[i]
    vol += b.volume[i]
    return vol > 0 ? pv / vol : null
  })
}

// ---------------------------------------------------------------- oscillators

export function macd(close: number[], fast: number, slow: number, signal: number) {
  const line = zip(ema(close, fast), ema(close, slow), (f, s) => f - s)
  const sig = ema(line, signal)
  return { macd: line, signal: sig, hist: zip(line, sig, (m, s) => m - s) }
}

/** RSI with Wilder smoothing. */
export function rsi(close: number[], n: number): Series {
  const gains: Series = close.map((c, i) => (i === 0 ? null : Math.max(c - close[i - 1], 0)))
  const losses: Series = close.map((c, i) => (i === 0 ? null : Math.max(close[i - 1] - c, 0)))
  return zip(rma(gains, n), rma(losses, n), (g, l) => (l === 0 ? (g === 0 ? 50 : 100) : 100 - 100 / (1 + g / l)))
}

/** Slow stochastic: fast %K over n, smoothed by k → slow %K, then d → slow %D. */
export function stochasticSlow(b: Bars, n: number, k: number, d: number) {
  const hh = highest(b.high, n)
  const ll = lowest(b.low, n)
  const fastK: Series = b.close.map((c, i) => {
    const h = hh[i]
    const l = ll[i]
    if (h === null || l === null) return null
    return h === l ? 50 : ((c - l) / (h - l)) * 100
  })
  const slowK = sma(fastK, k)
  return { k: slowK, d: sma(slowK, d) }
}

/** DMI (+DI / -DI) and ADX, Wilder smoothing. */
export function dmi(b: Bars, n: number, adxN: number) {
  const len = b.close.length
  const plusDM: Series = new Array(len).fill(null)
  const minusDM: Series = new Array(len).fill(null)
  const tr: Series = new Array(len).fill(null)
  const fullTr = trueRange(b.high, b.low, b.close)
  for (let i = 1; i < len; i++) {
    const upMove = b.high[i] - b.high[i - 1]
    const downMove = b.low[i - 1] - b.low[i]
    plusDM[i] = upMove > downMove && upMove > 0 ? upMove : 0
    minusDM[i] = downMove > upMove && downMove > 0 ? downMove : 0
    tr[i] = fullTr[i]
  }
  const atr = rma(tr, n)
  const plusDI = zip(rma(plusDM, n), atr, (dm, a) => (a === 0 ? 0 : (100 * dm) / a))
  const minusDI = zip(rma(minusDM, n), atr, (dm, a) => (a === 0 ? 0 : (100 * dm) / a))
  const dx = zip(plusDI, minusDI, (p, m) => (p + m === 0 ? 0 : (100 * Math.abs(p - m)) / (p + m)))
  return { plusDI, minusDI, adx: rma(dx, adxN) }
}

export function cci(b: Bars, n: number): Series {
  const tp = b.close.map((c, i) => (b.high[i] + b.low[i] + c) / 3)
  const mean = sma(tp, n)
  return tp.map((v, i) => {
    const m = mean[i]
    if (m === null) return null
    let dev = 0
    for (let j = i - n + 1; j <= i; j++) dev += Math.abs(tp[j] - m)
    dev /= n
    return dev === 0 ? 0 : (v - m) / (0.015 * dev)
  })
}

export function williamsR(b: Bars, n: number): Series {
  const hh = highest(b.high, n)
  const ll = lowest(b.low, n)
  return b.close.map((c, i) => {
    const h = hh[i]
    const l = ll[i]
    if (h === null || l === null) return null
    return h === l ? -50 : ((h - c) / (h - l)) * -100
  })
}

export function atr(b: Bars, n: number): Series {
  return rma(trueRange(b.high, b.low, b.close), n)
}

// ---------------------------------------------------------------- volume

export function obv(b: Bars): Series {
  let acc = 0
  return b.close.map((c, i) => {
    if (i > 0) acc += c > b.close[i - 1] ? b.volume[i] : c < b.close[i - 1] ? -b.volume[i] : 0
    return acc
  })
}

/** Money Flow Index. */
export function mfi(b: Bars, n: number): Series {
  const tp = b.close.map((c, i) => (b.high[i] + b.low[i] + c) / 3)
  return tp.map((_, i) => {
    if (i < n) return null
    let pos = 0
    let neg = 0
    for (let j = i - n + 1; j <= i; j++) {
      const flow = tp[j] * b.volume[j]
      if (tp[j] > tp[j - 1]) pos += flow
      else if (tp[j] < tp[j - 1]) neg += flow
    }
    return neg === 0 ? (pos === 0 ? 50 : 100) : 100 - 100 / (1 + pos / neg)
  })
}

// ---------------------------------------------------------------- KR-style

/** 이격도: close as % of its n-period SMA (100 = on the average). */
export function disparity(close: number[], n: number): Series {
  return zip(close, sma(close, n), (c, m) => (m === 0 ? null : (c / m) * 100))
}

/** 투자심리선: % of up-closes in the last n bars. */
export function psychologicalLine(close: number[], n: number): Series {
  return close.map((_, i) => {
    if (i < n) return null
    let ups = 0
    for (let j = i - n + 1; j <= i; j++) if (close[j] > close[j - 1]) ups++
    return (ups / n) * 100
  })
}
