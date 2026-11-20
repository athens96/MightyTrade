import { api } from './client'
import type { Candle, Interval, Market } from './types'
import type { IndicatorConfig } from '../indicators/registry'

export type StrategyCategory = 'swing' | 'daytrade'

export interface StrategyParam {
  key: string
  label: string
  default: number
  min: number
  max: number
  step: number
  integer: boolean
}

export interface StrategyGuide {
  background?: string
  setup?: string[]
  execution?: string[]
  caution?: string
}

export interface StrategyMeta {
  id: string
  name: string
  category: StrategyCategory
  timeframe: Interval
  theory: string
  summary: string
  description: string
  guide?: StrategyGuide
  params: StrategyParam[]
}

export interface CostDefaults {
  capital: number
  currency: string
  commission_pct: number
  commission_source: 'toss' | 'default'
  tax_pct: number
  slippage_pct: number
}

type N = number | null

export interface Metrics {
  start_equity: number
  final_equity: number
  total_return_pct: N
  cagr_pct: N
  mdd_pct: N
  max_dd_days: N
  volatility_pct: N
  sharpe: N
  sortino: N
  calmar: N
  trades?: number
  win_rate_pct?: N
  avg_return_pct?: N
  avg_win_pct?: N
  avg_loss_pct?: N
  best_pct?: N
  worst_pct?: N
  profit_factor?: N
  avg_bars?: N
  exposure_pct?: N
}

export interface BacktestTrade {
  entry_time: number
  entry_price: number
  qty: number
  entry_reason: string
  exit_time: number | null
  exit_price: number | null
  exit_reason: string
  pnl: N
  return_pct: N
  bars: number
  open: boolean
}

export interface EquityPoint {
  time: number
  equity: number
  benchmark: number
  drawdown: number
}

export type Period = '1y' | '3y' | '5y' | '10y' | 'max'

export interface BacktestRequest {
  market: Market
  symbol: string
  strategy: string
  params: Record<string, number>
  period: Period
  capital: number
  commission_pct: number
  tax_pct: number
  slippage_pct: number
}

export interface BacktestResult {
  market: Market
  symbol: string
  strategy: string
  interval?: Interval
  params: Record<string, number>
  capital: number
  currency: string
  period: { start: number; end: number }
  start_index: number
  candles: Candle[]
  chart_indicators: IndicatorConfig[]
  warnings: string[]
  equity: EquityPoint[]
  metrics: Metrics
  benchmark: Metrics
  trades: BacktestTrade[]
}

export interface AxisRange {
  key: string
  start: number
  stop: number
  step: number
}

export interface GridRequest extends BacktestRequest {
  x: AxisRange
  y?: AxisRange
  objective: string
  min_trades: number
}

export interface Axis {
  key: string
  label: string
  values: number[]
}

export type CellMetrics = Pick<
  Metrics,
  'total_return_pct' | 'cagr_pct' | 'mdd_pct' | 'sharpe' | 'sortino' | 'calmar' | 'win_rate_pct' | 'profit_factor'
> & { trades: number }

export type GridCell =
  | { invalid: string }
  | { params: Record<string, number>; metrics: CellMetrics; score: N; eligible: boolean; neighbors: number; neighbor_avg: N }

export interface GridPick {
  x: number
  y: number
  params: Record<string, number>
  metrics: CellMetrics
  score: number
  neighbor_avg: N
}

interface RunContext {
  market: Market
  symbol: string
  strategy: string
  capital: number
  currency: string
  period: { start: number; end: number }
  warnings: string[]
  elapsed_sec?: number
}

export interface OptimizeResult extends RunContext {
  x: Axis
  y: Axis | null
  objective: string
  min_trades: number
  cells: (GridCell | null)[][]
  best: GridPick | null
  robust: GridPick | null
  benchmark: Metrics
}

export interface WalkForwardWindow {
  is_start: number
  is_end: number
  oos_start: number
  oos_end: number
  params: Record<string, number> | null
  is_score: N
  is_cagr_pct: N
  oos: Pick<Metrics, 'total_return_pct' | 'cagr_pct' | 'mdd_pct' | 'sharpe'> & { trades: number } | null
}

export interface WalkForwardRequest extends GridRequest {
  is_years: number
  oos_months: number
  mode: 'rolling' | 'anchored'
}

export interface WalkForwardResult extends RunContext {
  x: Axis
  y: Axis | null
  objective: string
  is_years: number
  oos_months: number
  mode: 'rolling' | 'anchored'
  windows: WalkForwardWindow[]
  equity: EquityPoint[]
  metrics: Metrics
  benchmark: Metrics
  trades: BacktestTrade[]
  efficiency: N
  mean_is_cagr_pct: N
}

type Pcts = { p5: number; p25: number; p50: number; p75: number; p95: number }

export interface MonteCarloRequest extends BacktestRequest {
  runs: number
  method: 'bootstrap' | 'shuffle'
}

export interface MonteCarloResult extends RunContext {
  params: Record<string, number>
  backtest_metrics: Metrics
  runs: number
  method: 'bootstrap' | 'shuffle'
  trades: number
  final_return_pct: Pcts
  cagr_pct: { [K in keyof Pcts]: number | null }
  mdd_pct: Pcts
  prob_loss_pct: number
  prob_mdd_worse_pct: number
  original: { final_return_pct: number; mdd_pct: number; final_rank_pct: number; mdd_rank_pct: number; path: number[] }
  path_index: number[]
  bands: Record<keyof Pcts, number[]>
  hist_final: { edges: number[]; counts: number[] }
  hist_mdd: { edges: number[]; counts: number[] }
}

export type RebalanceMode = 'monthly' | 'quarterly' | 'yearly' | 'drift'

export interface PortfolioAsset {
  symbol: string
  name?: string
  role: 'offensive' | 'defensive'
  weight: number
}

export interface PortfolioModelMeta {
  id: string
  name: string
  theory: string
  summary: string
  description: string
  guide?: StrategyGuide
  uses_roles: boolean
  uses_weights: boolean
  default_rebalance: RebalanceMode
  params: StrategyParam[]
  presets: Record<Market, PortfolioAsset[]>
}

export interface PortfolioRebalanceEntry {
  time: number
  date: string
  reason: string
  trades: number
  turnover_pct: number
  weights: Record<string, number>
}

export interface PortfolioBacktestRequest {
  market: Market
  model: string
  assets: PortfolioAsset[]
  params: Record<string, number>
  rebalance: RebalanceMode
  drift_pct: number
  period: Period
  capital: number
  commission_pct: number
  tax_pct: number
  slippage_pct: number
}

export interface PortfolioBacktestResult {
  market: Market
  model: string
  assets: PortfolioAsset[]
  params: Record<string, number>
  rebalance: RebalanceMode
  drift_pct: number
  capital: number
  currency: string
  period: { start: number; end: number }
  warnings: string[]
  equity: EquityPoint[]
  metrics: Metrics & { rebalances: number }
  benchmark: Metrics
  rebalances: PortfolioRebalanceEntry[]
  current_weights: Record<string, number>
}

export const quantApi = {
  strategies: () => api<{ items: StrategyMeta[]; objectives: Record<string, string> }>('/quant/strategies'),
  optimize: (req: GridRequest) => api<OptimizeResult>('/quant/optimize', { method: 'POST', json: req }),
  walkforward: (req: WalkForwardRequest) => api<WalkForwardResult>('/quant/walkforward', { method: 'POST', json: req }),
  montecarlo: (req: MonteCarloRequest) => api<MonteCarloResult>('/quant/montecarlo', { method: 'POST', json: req }),
  defaults: (market: Market) => api<CostDefaults>(`/quant/defaults?market=${market}`),
  backtest: (req: BacktestRequest) => api<BacktestResult>('/quant/backtest', { method: 'POST', json: req }),
  portfolioModels: () => api<{ items: PortfolioModelMeta[]; rebalance_modes: Record<RebalanceMode, string> }>('/quant/portfolio/models'),
  portfolioBacktest: (req: PortfolioBacktestRequest) => api<PortfolioBacktestResult>('/quant/portfolio/backtest', { method: 'POST', json: req }),
}
