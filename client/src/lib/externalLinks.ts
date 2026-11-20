import type { Market } from '../api/types'

/** External Investing.com search page for a KR or US symbol. */
export function investingUrl(_market: Market, symbol: string): string {
  return `https://kr.investing.com/search/?q=${encodeURIComponent(symbol.trim())}`
}
