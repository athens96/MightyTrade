// Numeric building blocks. Every series is index-aligned with the candles; `null` = not enough data yet.

export type Series = (number | null)[]

/** Simple moving average. A window containing any null yields null. */
export function sma(values: Series, n: number): Series {
  const out: Series = new Array(values.length).fill(null)
  let sum = 0
  let valid = 0 // consecutive non-null values ending at i
  for (let i = 0; i < values.length; i++) {
    const v = values[i]
    if (v === null) {
      sum = 0
      valid = 0
      continue
    }
    sum += v
    valid++
    if (valid > n) {
      sum -= values[i - n] as number
      valid = n
    }
    if (valid === n) out[i] = sum / n
  }
  return out
}

/** Exponential moving average (alpha = 2/(n+1)), seeded with the SMA of the first n values. */
export function ema(values: Series, n: number): Series {
  return smoothed(values, n, 2 / (n + 1))
}

/** Wilder's smoothing (RMA, alpha = 1/n), used by RSI, ATR, DMI/ADX. Seeded with an SMA. */
export function rma(values: Series, n: number): Series {
  return smoothed(values, n, 1 / n)
}

function smoothed(values: Series, n: number, alpha: number): Series {
  const out: Series = new Array(values.length).fill(null)
  let prev: number | null = null
  let seedSum = 0
  let seedCount = 0
  for (let i = 0; i < values.length; i++) {
    const v = values[i]
    if (v === null) {
      // leading nulls (e.g. MACD line warm-up) are skipped; a gap after start resets the average
      if (prev !== null) {
        prev = null
        seedSum = 0
        seedCount = 0
      }
      continue
    }
    if (prev === null) {
      seedSum += v
      seedCount++
      if (seedCount === n) {
        prev = seedSum / n
        out[i] = prev
      }
      continue
    }
    prev = alpha * v + (1 - alpha) * prev
    out[i] = prev
  }
  return out
}

/** Population standard deviation over a rolling window (same convention as most charting tools). */
export function stdev(values: Series, n: number): Series {
  const mean = sma(values, n)
  return values.map((_, i) => {
    const m = mean[i]
    if (m === null) return null
    let acc = 0
    for (let j = i - n + 1; j <= i; j++) acc += ((values[j] as number) - m) ** 2
    return Math.sqrt(acc / n)
  })
}

/** Highest value over the last n bars (inclusive). */
export function highest(values: number[], n: number): Series {
  return values.map((_, i) => (i < n - 1 ? null : Math.max(...values.slice(i - n + 1, i + 1))))
}

/** Lowest value over the last n bars (inclusive). */
export function lowest(values: number[], n: number): Series {
  return values.map((_, i) => (i < n - 1 ? null : Math.min(...values.slice(i - n + 1, i + 1))))
}

/** Element-wise op on two series, null-propagating. */
export function zip(a: Series, b: Series, f: (x: number, y: number) => number | null): Series {
  return a.map((x, i) => {
    const y = b[i]
    return x === null || y === null ? null : f(x, y)
  })
}

export function map(a: Series, f: (x: number, i: number) => number | null): Series {
  return a.map((x, i) => (x === null ? null : f(x, i)))
}

/** True range: max(high-low, |high-prevClose|, |low-prevClose|). First bar uses high-low. */
export function trueRange(high: number[], low: number[], close: number[]): number[] {
  return high.map((h, i) =>
    i === 0 ? h - low[i] : Math.max(h - low[i], Math.abs(h - close[i - 1]), Math.abs(low[i] - close[i - 1])),
  )
}
