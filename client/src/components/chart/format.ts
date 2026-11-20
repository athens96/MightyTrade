import type { Market } from '../../api/types'
import type { ValueFormat } from '../../indicators/registry'


/** KRW: integer with thousands separators. USD: 2 decimals (4 under $1). */
export function priceFormatter(market: Market) {
  return (p: number) =>
    market === 'KR'
      ? Math.round(p).toLocaleString('ko-KR')
      : p.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: Math.abs(p) < 1 ? 4 : 2 })
}

/** Korean units: 1,234 / 731만 / 1,007만 / 1.23억 / 2.1조. */
export function compact(v: number): string {
  const a = Math.abs(v)
  if (a >= 1e12) return `${(v / 1e12).toFixed(2)}조`
  if (a >= 1e8) return `${(v / 1e8).toFixed(2)}억`
  if (a >= 1e4) return `${Math.round(v / 1e4).toLocaleString('ko-KR')}만`
  return Math.round(v).toLocaleString('ko-KR')
}

export function valueFormatter(format: ValueFormat | undefined, market: Market) {
  if (format === 'price') return priceFormatter(market)
  if (format === 'volume') return compact
  return (v: number) => v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
}

/** lightweight-charts priceFormat option for a series. */
export function priceFormatOption(format: ValueFormat | undefined, market: Market) {
  if (format === 'number') return { type: 'price' as const, precision: 2, minMove: 0.01 }
  return {
    type: 'custom' as const,
    formatter: valueFormatter(format, market),
    minMove: format === 'volume' || market === 'KR' ? 1 : 0.01,
  }
}
