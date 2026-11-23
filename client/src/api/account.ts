import { api } from './client'
import type { Market } from './types'

type N = number | null
export type KrwUsd = { krw: N; usd: N }

export interface Holding {
  symbol: string
  name: string
  marketCountry: string
  currency: 'KRW' | 'USD'
  quantity: number
  lastPrice: number
  averagePurchasePrice: number
  marketValue: { purchaseAmount: number; amount: number; amountAfterCost: number }
  profitLoss: { amount: number; amountAfterCost: number; ratePct: number; rateAfterCostPct: number }
  dailyProfitLoss: { amount: number; ratePct: number }
  cost: { commission: number; tax: number }
}

export interface AccountSummary {
  broker?: 'toss' | 'kiwoom'
  kiwoom_mock?: boolean | null
  holdings: {
    totalPurchaseAmount: KrwUsd
    marketValue: { amount: KrwUsd; amountAfterCost: KrwUsd }
    profitLoss: { amount: KrwUsd; amountAfterCost: KrwUsd; ratePct: N; rateAfterCostPct: N }
    dailyProfitLoss: { amount: KrwUsd; ratePct: N }
    items: Holding[]
  }
  buying_power: { KRW: N; USD: N }
}

export interface ConditionalLeg {
  type: 'STOP' | 'PROFIT_RATE'
  status: string
  orderSide: 'BUY' | 'SELL'
  triggerPrice?: N
  targetProfitRate?: N
  orderPrice?: N
  triggeredOrderId?: string | null
}

export interface ConditionalOrder {
  conditionalOrderId: string
  type: 'SINGLE' | 'OCO' | 'OTO'
  status: 'WATCHING' | 'PAUSED' | 'ORDERING' | 'ORDERED' | 'COMPLETED' | 'EXPIRED'
  symbol: string
  market: Market
  quantity: number
  orderType: 'LIMIT' | 'MARKET'
  expireDate?: string
  first: ConditionalLeg
  second?: ConditionalLeg
  createdAt: string
}

export interface OcoRequest {
  market: Market
  symbol: string
  quantity: number
  take_profit: { trigger: number; price: number }
  stop_loss: { trigger: number; price: number }
  expire_date?: string
  confirm_high_value?: boolean
  dry_run?: boolean
  otp_code?: string
}

export interface OcoResponse {
  dry_run: boolean
  payload: {
    quantity: string
    expireDate: string
    first: { triggerPrice: string; orderPrice: string }
    second: { triggerPrice: string; orderPrice: string }
  }
  checks: { current_price: number; sellable_quantity: number; order_value: number; take_profit_pct: number; stop_loss_pct: number }
  result?: { conditionalOrderId: string }
}

export const accountApi = {
  summary: () => api<AccountSummary>('/account/summary'),
  conditionalOrders: (status: 'OPEN' | 'CLOSED') => api<{ items: ConditionalOrder[] }>(`/account/conditional-orders?status=${status}`),
  oco: (req: OcoRequest) => api<OcoResponse>('/account/conditional-orders/oco', { method: 'POST', json: req }),
  cancelConditional: (id: string) => api<{ ok: true }>(`/account/conditional-orders/${encodeURIComponent(id)}`, { method: 'DELETE' }),
}
