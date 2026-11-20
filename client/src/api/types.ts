export type Market = 'KR' | 'US'
export type Interval = '1m' | '5m' | '15m' | '1h' | '1d' | '1w'

export interface User {
  id: string
  email: string
  name: string
  picture?: string
  otp_enabled: boolean
}

export interface Candle {
  time: number
  open: number
  high: number
  low: number
  close: number
  volume: number
}

export interface Quote {
  market: Market
  symbol: string
  price: number
  change: number
  change_pct: number
  currency: string
  time: number
}

export interface WatchItem {
  market: Market
  symbol: string
  name: string
}

export interface Instrument {
  market: Market
  symbol: string
  name: string
  exchange: string
  english_name: string
}
