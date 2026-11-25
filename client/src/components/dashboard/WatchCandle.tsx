import type { Market, WatchQuote } from '../../api/types'
import { priceFormatter } from '../chart/format'

const W = 16
const H = 30
const PAD = 2.5
const BODY_W = 8
const BODY_X = (W - BODY_W) / 2
const WICK_X = W / 2

export interface WatchCandleGeom {
  wickTop: number
  wickBottom: number
  bodyTop: number
  bodyHeight: number
  prevCloseY: number | null
  kind: 'bull' | 'bear' | 'flat'
  color: string
  intradayPosPct: number | null
  open: number
  high: number
  low: number
  close: number
  prevClose: number | null
}

/**
 * Pure SVG coordinate calculator for a compact today's live candlestick icon.
 * Uses a minimum 1% reference span so tiny 0.05% fluctuations do not distort into a full-height bar.
 */
export function computeWatchCandle(q?: WatchQuote): WatchCandleGeom | null {
  if (!q || q.error || q.price === undefined || q.price <= 0) return null
  const close = q.price
  const prevClose =
    q.prev_close !== undefined && q.prev_close > 0
      ? q.prev_close
      : q.change !== undefined && close - q.change > 0
        ? close - q.change
        : null
  const open = q.open !== undefined && q.open > 0 ? q.open : prevClose ?? close
  const high = Math.max(q.high !== undefined && q.high > 0 ? q.high : close, open, close)
  const low = Math.min(q.low !== undefined && q.low > 0 ? q.low : close, open, close)

  let minP = prevClose !== null ? Math.min(low, prevClose) : low
  let maxP = prevClose !== null ? Math.max(high, prevClose) : high
  const ref = prevClose ?? open
  const minSpan = Math.max(ref * 0.01, 1e-6)
  if (maxP - minP < minSpan) {
    const mid = (maxP + minP) / 2
    minP = mid - minSpan / 2
    maxP = mid + minSpan / 2
  }
  const span = maxP - minP
  const usableH = H - PAD * 2
  const toY = (p: number) => PAD + ((maxP - p) / span) * usableH

  const wickTop = toY(high)
  const wickBottom = toY(low)
  const yOpen = toY(open)
  const yClose = toY(close)
  const rawBodyTop = Math.min(yOpen, yClose)
  const rawBodyH = Math.abs(yClose - yOpen)
  const bodyHeight = Math.max(2, rawBodyH)
  const bodyTop = rawBodyH < 2 ? Math.min(Math.max(PAD, rawBodyTop - (2 - rawBodyH) / 2), H - PAD - 2) : rawBodyTop

  const kind: 'bull' | 'bear' | 'flat' = close > open ? 'bull' : close < open ? 'bear' : 'flat'
  const color = kind === 'bull' ? '#e5484d' : kind === 'bear' ? '#3e63dd' : '#8b949e'
  const intradayPosPct = high > low ? Math.round(((close - low) / (high - low)) * 100) : null

  return {
    wickTop,
    wickBottom,
    bodyTop,
    bodyHeight,
    prevCloseY: prevClose !== null ? toY(prevClose) : null,
    kind,
    color,
    intradayPosPct,
    open,
    high,
    low,
    close,
    prevClose,
  }
}

export default function WatchCandle({ q, market }: { q?: WatchQuote; market: Market }) {
  const g = computeWatchCandle(q)
  if (!g) {
    return <span className="wl-candle wl-candle-empty" aria-hidden="true" />
  }
  const fmt = priceFormatter(market)
  const kindLabel = g.kind === 'bull' ? '양봉 (시가 대비 상승)' : g.kind === 'bear' ? '음봉 (시가 대비 하락)' : '보합 (도지)'
  const posText = g.intradayPosPct !== null ? ` · 장중 위치 ${g.intradayPosPct}%` : ''
  const prevText = g.prevClose !== null ? `\n전일종가 ${fmt(g.prevClose)} (점선)` : ''
  const tip = `당일 일봉 캔들 · ${kindLabel}${posText}\n시가 ${fmt(g.open)} · 고가 ${fmt(g.high)} · 저가 ${fmt(g.low)} · 현재가 ${fmt(g.close)}${prevText}`

  return (
    <span className="wl-candle" title={tip} aria-label={kindLabel}>
      <svg width={W} height={H} viewBox={`0 0 ${W} ${H}`} role="img" aria-hidden="true">
        {g.prevCloseY !== null && (
          <line
            x1={1}
            x2={W - 1}
            y1={g.prevCloseY}
            y2={g.prevCloseY}
            stroke="#484f58"
            strokeWidth={1}
            strokeDasharray="2 1.5"
          />
        )}
        <line
          x1={WICK_X}
          x2={WICK_X}
          y1={g.wickTop}
          y2={g.wickBottom}
          stroke={g.color}
          strokeWidth={1.5}
          strokeLinecap="round"
        />
        <rect
          x={BODY_X}
          y={g.bodyTop}
          width={BODY_W}
          height={g.bodyHeight}
          rx={1}
          fill={g.color}
        />
      </svg>
    </span>
  )
}
