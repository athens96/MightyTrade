import { api } from './client'
import type { Market } from './types'

type N = number | null

export interface TradingStatus {
  unlocked: boolean
  unlocked_until: string | null
  today_orders: number
  limits: { max_orders_per_day: number; max_order_krw: number; max_order_usd: number; unlock_minutes: number }
}

export type Side = 'BUY' | 'SELL'
export type OrderType = 'LIMIT' | 'MARKET'

export interface OrderRequest {
  market: Market
  symbol: string
  side: Side
  order_type: OrderType
  quantity: number
  price?: number
  time_in_force?: 'DAY' | 'OPG' | 'CLS'
  confirm_high_value?: boolean
  dry_run?: boolean
}

export interface OrderPreview {
  dry_run: boolean
  market: Market
  payload: { side: Side; orderType: OrderType; quantity: string; price?: string; timeInForce: string }
  checks: { current_price: number; price: N; estimated_value: number; currency: string; session: { open: boolean; label: string } | null }
  warnings: string[]
  result?: { orderId: string }
}

export interface Order {
  orderId: string
  symbol: string
  side: Side
  orderType: OrderType
  timeInForce: string
  status: string
  price?: N
  quantity: number
  currency: 'KRW' | 'USD'
  orderedAt: string
  canceledAt?: string | null
  execution: { filledQuantity: number; averageFilledPrice: N; filledAmount: N; commission: N; tax: N; filledAt: string | null }
}

export const tradingApi = {
  status: () => api<TradingStatus>('/trading/status'),
  unlock: (otp_code: string) => api<TradingStatus>('/trading/unlock', { method: 'POST', json: { otp_code } }),
  lock: () => api<TradingStatus>('/trading/lock', { method: 'POST' }),
  order: (req: OrderRequest) => api<OrderPreview>('/trading/orders', { method: 'POST', json: req }),
  orders: (status: 'OPEN' | 'CLOSED') => api<{ items: Order[] }>(`/trading/orders?status=${status}`),
  cancel: (id: string) => api<{ result: { orderId: string } }>(`/trading/orders/${encodeURIComponent(id)}/cancel`, { method: 'POST' }),
  modify: (id: string, body: { price?: number; quantity?: number; order_type?: OrderType }) =>
    api<{ result: { orderId: string } }>(`/trading/orders/${encodeURIComponent(id)}/modify`, { method: 'POST', json: body }),
}

export const ORDER_STATUS: Record<string, string> = {
  PENDING: '접수', PARTIAL_FILLED: '부분 체결', FILLED: '체결', CANCELED: '취소', REJECTED: '거부', PENDING_CANCEL: '취소 중',
  PENDING_REPLACE: '정정 중', REPLACED: '정정됨', CANCEL_REJECTED: '취소 거부', REPLACE_REJECTED: '정정 거부',
}
