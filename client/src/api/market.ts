import { api } from './client'
import type { Candle, Instrument, Interval, Market, Quote, WatchItem, WatchQuote } from './types'

export const marketApi = {
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
