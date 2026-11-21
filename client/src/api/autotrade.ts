import { api } from './client'
import type { MarketTimingRules, PortfolioAsset, RebalanceMode, RiskRules } from './quant'
import type { Interval, Market } from './types'

export type BotKind = 'single' | 'rebalance' | 'rotation' | 'dca'
export type BotCategory = 'swing' | 'daytrade' | 'rebalance' | 'rotation' | 'dca'
export type RotationCadence = 'weekly' | 'biweekly' | 'monthly'
export type DcaCadence = 'daily' | 'weekly' | 'biweekly' | 'monthly'

export interface DcaStats {
  steps: number
  total_invested: number
  last_dca_date: string | null
  smart_avg_price: number | null
  regular_avg_price: number | null
  saved_pct: number | null
}

export interface BotTrade {
  entry_date: string
  entry_price: number
  qty: number
  entry_reason: string
  exit_date: string | null
  exit_price: number | null
  exit_reason: string
  pnl: number | null
  return_pct: number | null
}

export interface BotRebalanceLog {
  date: string
  reason: string
  trades: number
  weights: Record<string, number>
}

export type BotMode = 'paper' | 'semi' | 'live'

export interface PendingApprovalOrder {
  symbol: string
  side: 'BUY' | 'SELL'
  kind: string
  qty: number
  signal_price: number
  level: number | null
  reason: string
  text: string
}

export interface PendingApproval {
  id: string
  date: string
  created_at: string
  expires_at: string
  reason: string
  orders: PendingApprovalOrder[]
}

export interface GuardRules {
  max_drawdown_pct?: number
  max_consecutive_losses?: number
  cooldown_days?: number
  daily_loss_limit_pct?: number
}

export interface Bot {
  id: string
  kind: BotKind
  category: BotCategory
  timeframe: Interval
  name: string
  market: Market
  symbol: string
  strategy?: string
  model?: string
  assets?: PortfolioAsset[]
  preset?: string
  universe?: string
  custom_symbols?: string[]
  top_n?: number
  hold_buffer?: number
  exclude_warnings?: boolean
  rebalance?: RebalanceMode | RotationCadence | DcaCadence
  drift_pct?: number
  buy_only?: boolean
  weekday?: number
  base_amount?: number
  smart_scale?: boolean
  dip_mult?: number
  peak_mult?: number
  params: Record<string, number>
  risk?: RiskRules
  guard?: GuardRules
  market_timing?: MarketTimingRules
  capital: number
  mode: BotMode
  enabled: boolean
  created_at: string
  updated_at: string
  state: {
    position: { qty: number; entry_price: number; entry_date: string | null }
    positions?: Record<string, { qty: number; entry_price: number; name?: string }>
    target_weights?: Record<string, number>
    selected_picks?: { symbol: string; name: string; rank: number; score: number }[]
    last_prices?: Record<string, number>
    last_rebalance_date?: string | null
    rebalances?: BotRebalanceLog[]
    pending: { side: string; symbol?: string; kind?: string; level?: number; active_date?: string; date?: string }[]
    pending_approval?: PendingApproval | null
    last_run_date: string | null
    last_price: number | null
    trades: BotTrade[]
    equity: { date: string; value: number }[]
    tripped?: { at: string; date: string; reason: string } | null
    cooldown_until?: string | null
    dca_stats?: DcaStats
    cash?: number
    equity_now?: number
  }
}

export interface BotEvent {
  id: string
  at: string
  kind: string
  level: 'info' | 'warn' | 'error'
  message: string
  mode: BotMode
}

export interface Evaluation {
  date: string
  bar_date: string
  due?: boolean
  reason?: string | null
  position: { qty: number; entry_price: number }
  cash: number
  today_open: number | null
  last: number
  current_weights?: Record<string, number>
  target_weights?: Record<string, number>
  orders: { side: string; kind: string; symbol?: string; text: string }[]
  planned_orders?: { side: string; kind: string; symbol?: string; text: string }[]
}

export interface BotInput {
  kind?: BotKind
  name?: string
  market: Market
  symbol?: string
  strategy?: string
  model?: string
  assets?: PortfolioAsset[]
  preset?: string
  universe?: string
  custom_symbols?: string[] | string
  top_n?: number
  hold_buffer?: number
  exclude_warnings?: boolean
  rebalance?: RebalanceMode | RotationCadence | DcaCadence
  drift_pct?: number
  buy_only?: boolean
  weekday?: number
  base_amount?: number
  smart_scale?: boolean
  dip_mult?: number
  peak_mult?: number
  params?: Record<string, number>
  risk?: RiskRules
  guard?: GuardRules
  market_timing?: MarketTimingRules
  capital: number
}

export const autotradeApi = {
  status: () => api<{ halted: boolean; scheduler: { running: boolean; last_tick: string | null; error: string | null } }>('/autotrade/status'),
  halt: () => api<{ halted: boolean; cancelled: number }>('/autotrade/halt', { method: 'POST' }),
  resume: (otp_code: string) => api<{ halted: boolean }>('/autotrade/resume', { method: 'POST', json: { otp_code } }),
  bots: () => api<{ items: Bot[] }>('/autotrade/bots'),
  bot: (id: string) => api<{ bot: Bot; events: BotEvent[] }>(`/autotrade/bots/${id}`),
  create: (b: BotInput) => api<{ bot: Bot }>('/autotrade/bots', { method: 'POST', json: b }),
  update: (id: string, b: Partial<BotInput>) => api<{ bot: Bot }>(`/autotrade/bots/${id}`, { method: 'PUT', json: b }),
  remove: (id: string) => api<{ ok: true }>(`/autotrade/bots/${id}`, { method: 'DELETE' }),
  enable: (id: string) => api<{ bot: Bot }>(`/autotrade/bots/${id}/enable`, { method: 'POST' }),
  disable: (id: string) => api<{ bot: Bot }>(`/autotrade/bots/${id}/disable`, { method: 'POST' }),
  mode: (id: string, mode: BotMode, otp_code?: string) =>
    api<{ bot: Bot }>(`/autotrade/bots/${id}/mode`, { method: 'POST', json: { mode, otp_code } }),
  approve: (id: string, otp_code?: string) =>
    api<{ result: { sent: number }; bot: Bot }>(`/autotrade/bots/${id}/approve`, { method: 'POST', json: otp_code ? { otp_code } : {} }),
  skip: (id: string) => api<{ bot: Bot }>(`/autotrade/bots/${id}/skip`, { method: 'POST' }),
  evaluate: (id: string) => api<Evaluation>(`/autotrade/bots/${id}/evaluate`, { method: 'POST' }),
  run: (id: string) => api<{ result: unknown; bot: Bot }>(`/autotrade/bots/${id}/run`, { method: 'POST' }),
  reset: (id: string) => api<{ bot: Bot }>(`/autotrade/bots/${id}/reset`, { method: 'POST' }),
}

export const PREFILL_KEY = 'mt.bot.prefill'
