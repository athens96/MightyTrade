import { api } from './client'
import type { PortfolioAsset, RebalanceMode, RiskRules } from './quant'
import type { Interval, Market } from './types'

export type BotKind = 'single' | 'rebalance'
export type BotCategory = 'swing' | 'daytrade' | 'rebalance'

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
  rebalance?: RebalanceMode
  drift_pct?: number
  params: Record<string, number>
  risk?: RiskRules
  guard?: GuardRules
  capital: number
  mode: 'paper' | 'live'
  enabled: boolean
  created_at: string
  updated_at: string
  state: {
    position: { qty: number; entry_price: number; entry_date: string | null }
    positions?: Record<string, { qty: number; entry_price: number }>
    target_weights?: Record<string, number>
    last_prices?: Record<string, number>
    last_rebalance_date?: string | null
    rebalances?: BotRebalanceLog[]
    pending: { side: string; symbol?: string; kind?: string; level?: number; active_date?: string; date?: string }[]
    last_run_date: string | null
    last_price: number | null
    trades: BotTrade[]
    equity: { date: string; value: number }[]
    tripped?: { at: string; date: string; reason: string } | null
    cooldown_until?: string | null
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
  mode: 'paper' | 'live'
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
  rebalance?: RebalanceMode
  drift_pct?: number
  params?: Record<string, number>
  risk?: RiskRules
  guard?: GuardRules
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
  mode: (id: string, mode: 'paper' | 'live', otp_code?: string) =>
    api<{ bot: Bot }>(`/autotrade/bots/${id}/mode`, { method: 'POST', json: { mode, otp_code } }),
  evaluate: (id: string) => api<Evaluation>(`/autotrade/bots/${id}/evaluate`, { method: 'POST' }),
  run: (id: string) => api<{ result: unknown; bot: Bot }>(`/autotrade/bots/${id}/run`, { method: 'POST' }),
  reset: (id: string) => api<{ bot: Bot }>(`/autotrade/bots/${id}/reset`, { method: 'POST' }),
}

export const PREFILL_KEY = 'mt.bot.prefill'
