import { api } from './client'
import type { Candle, Instrument, Interval, Market, Quote, WatchItem, WatchQuote } from './types'

export interface IndexQuote { symbol: string; price: number; change: number; change_pct: number }
export interface SessionStatus {
  open: boolean
  session: string | null
  label: string
  until?: string
  next?: { session: string; label: string; at: string } | null
}
export interface StockWarning { type: string; label: string; level: 'warning' | 'serious' | 'critical'; start?: string; end?: string }
export interface FlowRecord { date: string; individual: number | null; foreigner: number | null; institution: number | null; foreign_holding_pct: number | null }
export interface BookLevel { price: number; volume: number }
export interface OrderBook { timestamp?: string | null; currency: string; asks: BookLevel[]; bids: BookLevel[] }
export interface Tick { price: number; volume: number; timestamp: string; currency: string }

export const marketApi = {
  overview: () => api<{ indices: IndexQuote[]; status: Record<'KR' | 'US', SessionStatus> }>('/market/overview'),
  warnings: (market: Market, symbol: string) =>
    api<{ items: StockWarning[] }>(`/market/warnings?${new URLSearchParams({ market, symbol })}`),
  investorFlow: (symbol: string, count = 20) =>
    api<{ records: FlowRecord[] }>(`/market/investor-flow?${new URLSearchParams({ symbol, count: String(count) })}`),
  orderbook: (market: Market, symbol: string) =>
    api<{ orderbook: OrderBook; trades: Tick[] }>(`/market/orderbook?${new URLSearchParams({ market, symbol })}`),
  candles: (market: Market, symbol: string, interval: Interval, limit = 200) =>
    api<{ candles: Candle[]; provider: string }>(
      `/market/candles?${new URLSearchParams({ market, symbol, interval, limit: String(limit) })}`,
    ),
  /** Batch quotes for the watchlist; failed symbols come back with `error` instead of a price. */
  quotes: (market: Market, symbols: string[]) =>
    api<{ items: WatchQuote[] }>(`/market/quotes?${new URLSearchParams({ market, symbols: symbols.join(',') })}`),
  quote: (market: Market, symbol: string) =>
    api<{ quote: Quote }>(`/market/quote?${new URLSearchParams({ market, symbol })}`),
  search: (market: Market, q: string) =>
    api<{ items: Instrument[] }>(`/market/search?${new URLSearchParams({ market, q })}`),
  watchlist: () => api<{ items: WatchItem[] }>('/users/me/watchlist'),
  saveWatchlist: (items: WatchItem[]) =>
    api<{ items: WatchItem[] }>('/users/me/watchlist', { method: 'PUT', json: { items } }),
}
