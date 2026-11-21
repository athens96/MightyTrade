import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { marketApi } from '../api/market'
import {
  quantApi,
  type AxisRange,
  type BacktestRequest,
  type BacktestResult,
  type CostDefaults,
  type MarketTimingMode,
  type MonteCarloResult,
  type OptimizeResult,
  type Period,
  type PortfolioAsset,
  type PortfolioBacktestResult,
  type PortfolioModelMeta,
  type PortfolioRole,
  type RebalanceMode,
  type ScreenerPresetMeta,
  type ScreenerResult,
  type ScreenerUniverseMeta,
  type StrategyMeta,
  type StrategyParam,
  type WalkForwardResult,
} from '../api/quant'
import type { Instrument, Market, WatchItem } from '../api/types'
import { MOBILE, useMediaQuery } from '../hooks/useMediaQuery'
import BacktestView from '../components/quant/BacktestView'
import { FeatureGuideBox, StrategyGuideBox } from '../components/GuideBox'
import MonteCarloView from '../components/quant/MonteCarloView'
import OptimizeView from '../components/quant/OptimizeView'
import PortfolioView from '../components/quant/PortfolioView'
import ScreenerView from '../components/quant/ScreenerView'
import WalkForwardView from '../components/quant/WalkForwardView'
import { FEATURE_GUIDES } from '../lib/featureGuides'
import { getParamHint } from '../lib/paramHints'

type Mode = 'backtest' | 'optimize' | 'walkforward' | 'montecarlo' | 'portfolio' | 'screener'

const ROLE_LABELS: Record<PortfolioRole, string> = {
  offensive: '공격',
  defensive: '방어',
  canary: '감시병',
  core: '고정',
}

const MODES: { id: Mode; label: string; hint: string; tip: string }[] = [
  { id: 'backtest', label: '백테스트', hint: '설정 하나로 과거 성과 확인', tip: '선택한 종목과 전략 하나로 과거 몇 년간 매매했을 때의 수익률·낙폭·매매 내역을 확인합니다' },
  { id: 'optimize', label: '최적화', hint: '파라미터 조합을 격자로 비교', tip: '전략 숫자(예: 평균선 일수)를 여러 개로 바꿔가며 넓고 안정적인 구간을 격자 지도로 찾습니다' },
  { id: 'walkforward', label: '워크포워드', hint: '처음 보는 기간에도 통하는지 검증', tip: '과거 구간으로 공부한 설정이 처음 보는 미래 구간에서도 통하는지 실전처럼 검증합니다' },
  { id: 'montecarlo', label: '몬테카를로', hint: '매매 순서·구성의 운을 제거', tip: '매매 순서나 구성을 수천 번 무작위로 섞어 백테스트 성적이 단순한 운이었는지 확인합니다' },
  { id: 'portfolio', label: '포트폴리오', hint: '다중 ETF 자산배분·리밸런싱 검증', tip: '여러 주식·채권·금 ETF를 함께 담고 정기적으로 비중을 맞출 때의 장기 성과를 실험합니다' },
  { id: 'screener', label: '종목 발굴', hint: '퀀트 데이터로 유망 종목 자동 추출', tip: '수백 개 국내·미국 종목을 점수화해 지금 조건에 맞는 유망 종목을 자동으로 추려냅니다' },
]
const PERIODS: { id: Period; label: string }[] = [
  { id: '1y', label: '1년' },
  { id: '3y', label: '3년' },
  { id: '5y', label: '5년' },
  { id: '10y', label: '10년' },
  { id: 'max', label: '전체' },
]
const FORM_KEY = 'mt.quant.form.v2'

interface Form {
  mode: Mode
  market: Market
  symbol: string
  strategy: string
  period: Period
  params: Record<string, Record<string, number>> // per strategy
  ranges: Record<string, Record<string, AxisRange>> // per strategy, per param
  xKey: Record<string, string>
  yKey: Record<string, string> // '' = 1-D
  objective: string
  minTrades: number
  isYears: number
  oosMonths: number
  wfMode: 'rolling' | 'anchored'
  runs: number
  mcMethod: 'bootstrap' | 'shuffle'
  portfolioModel: string
  portfolioRebalance: RebalanceMode
  portfolioDriftPct: number
  portfolioBuyOnly: boolean
  portfolioParams: Record<string, Record<string, number>>
  portfolioAssets: Record<string, PortfolioAsset[]>
  screenerPreset: string
  screenerUniverse: Record<Market, string>
  screenerCustom: Record<Market, string>
  screenerStrategy: string
  screenerTopN: number
  screenerExcludeWarnings: boolean
  screenerStrictOnly: boolean
  stopLossPct: number
  takeProfitPct: number
  trailingStopPct: number
  mtMode: MarketTimingMode
  mtBenchmark: Record<Market, string>
}

const DEFAULT_FORM: Form = {
  mode: 'backtest',
  market: 'KR',
  symbol: '005930',
  strategy: 'ma_cross',
  period: '10y',
  params: {},
  ranges: {},
  xKey: {},
  yKey: {},
  objective: 'sharpe',
  minTrades: 5,
  isYears: 3,
  oosMonths: 12,
  wfMode: 'rolling',
  runs: 1000,
  mcMethod: 'bootstrap',
  portfolioModel: 'all_weather',
  portfolioRebalance: 'quarterly',
  portfolioDriftPct: 5,
  portfolioBuyOnly: false,
  portfolioParams: {},
  portfolioAssets: {},
  screenerPreset: 'momentum_quality',
  screenerUniverse: { KR: 'kr_all_150', US: 'us_sp100' },
  screenerCustom: { KR: '005930, 000660, 005380, 105560', US: 'AAPL, MSFT, NVDA, GOOGL' },
  screenerStrategy: 'rsi2',
  screenerTopN: 20,
  screenerExcludeWarnings: true,
  screenerStrictOnly: false,
  stopLossPct: 0,
  takeProfitPct: 0,
  trailingStopPct: 0,
  mtMode: 'off',
  mtBenchmark: { KR: '069500', US: 'SPY' },
}

const loadForm = (): Form => {
  try {
    return { ...DEFAULT_FORM, ...JSON.parse(localStorage.getItem(FORM_KEY) ?? '{}') }
  } catch {
    return DEFAULT_FORM
  }
}

/** ~7 values around the default, inside the parameter's bounds. */
function defaultRange(p: StrategyParam): AxisRange {
  let start = p.default ? p.default / 2 : p.min
  let stop = p.default ? p.default * 2 : p.min + (p.max - p.min) / 4
  start = Math.max(p.min, p.integer ? Math.round(start) : start)
  stop = Math.min(p.max, p.integer ? Math.round(stop) : stop)
  let step = (stop - start) / 6
  step = p.integer ? Math.max(1, Math.round(step)) : Math.max(p.step, Number(step.toPrecision(1)))
  return { key: p.key, start, stop, step }
}

export default function QuantPage() {
  const [form, setForm] = useState<Form>(loadForm)
  const [strategies, setStrategies] = useState<StrategyMeta[]>([])
  const [portfolioModels, setPortfolioModels] = useState<PortfolioModelMeta[]>([])
  const [screenerPresets, setScreenerPresets] = useState<ScreenerPresetMeta[]>([])
  const [screenerUniverses, setScreenerUniverses] = useState<Record<Market, ScreenerUniverseMeta[]>>({ KR: [], US: [] })
  const [dailyStrategies, setDailyStrategies] = useState<{ id: string; name: string }[]>([])
  const [rebalModes, setRebalModes] = useState<Record<RebalanceMode, string>>({
    monthly: '매월 초', quarterly: '분기 초', yearly: '매년 초', drift: '비중 편차 이탈 시',
  })
  const [objectives, setObjectives] = useState<Record<string, string>>({})
  const [watchlist, setWatchlist] = useState<WatchItem[]>([])
  const [symQuery, setSymQuery] = useState('')
  const [symHits, setSymHits] = useState<Instrument[]>([])
  const [costs, setCosts] = useState<CostDefaults | null>(null)
  const [running, setRunning] = useState(false)
  const isMobile = useMediaQuery(MOBILE)
  const [formOpen, setFormOpen] = useState(true) // mobile: the form folds away after a run to show results
  const [error, setError] = useState('')
  const [results, setResults] = useState<{
    backtest?: BacktestResult
    optimize?: OptimizeResult
    walkforward?: WalkForwardResult
    montecarlo?: MonteCarloResult
    portfolio?: PortfolioBacktestResult
    screener?: ScreenerResult
  }>({})

  useEffect(() => {
    quantApi
      .strategies()
      .then(({ items, objectives }) => {
        setStrategies(items)
        setObjectives(objectives)
      })
      .catch((e) => setError(e.message))
    quantApi
      .portfolioModels()
      .then(({ items, rebalance_modes }) => {
        setPortfolioModels(items)
        setRebalModes(rebalance_modes)
      })
      .catch(() => undefined)
    quantApi
      .screenerPresets()
      .then(({ presets, universes, daily_strategies }) => {
        setScreenerPresets(presets)
        setScreenerUniverses(universes)
        setDailyStrategies(daily_strategies)
      })
      .catch(() => undefined)
    marketApi.watchlist().then(({ items }) => setWatchlist(items)).catch(() => undefined)
  }, [])

  useEffect(() => localStorage.setItem(FORM_KEY, JSON.stringify(form)), [form])

  useEffect(() => {
    setCosts(null)
    setSymQuery('')
    setSymHits([])
    quantApi.defaults(form.market).then(setCosts).catch((e) => setError(e.message))
  }, [form.market])

  useEffect(() => {
    const q = symQuery.trim()
    if (!q) {
      setSymHits([])
      return
    }
    const t = setTimeout(() => {
      marketApi.search(form.market, q).then((r) => setSymHits(r.items.slice(0, 8))).catch(() => setSymHits([]))
    }, 220)
    return () => clearTimeout(t)
  }, [form.market, symQuery])

  const set = <K extends keyof Form>(key: K, value: Form[K]) => setForm((f) => ({ ...f, [key]: value }))
  const meta = strategies.find((s) => s.id === form.strategy) ?? strategies[0]
  const sid = meta?.id ?? ''

  const params = useMemo(() => {
    const saved = form.params[sid] ?? {}
    return Object.fromEntries((meta?.params ?? []).map((p) => [p.key, saved[p.key] ?? p.default]))
  }, [meta, sid, form.params])
  const rangeOf = (p: StrategyParam) => form.ranges[sid]?.[p.key] ?? defaultRange(p)
  const xKey = form.xKey[sid] ?? meta?.params[0]?.key ?? ''
  const yKey = form.yKey[sid] ?? meta?.params[1]?.key ?? ''
  const gridMode = form.mode === 'optimize' || form.mode === 'walkforward'
  const isPortfolio = form.mode === 'portfolio'
  const isScreener = form.mode === 'screener'
  const isIntraday = !isPortfolio && !isScreener && !!meta && meta.timeframe !== '1d'
  const wfBlockedByIntraday = form.mode === 'walkforward' && isIntraday

  const scPreset = screenerPresets.find((p) => p.id === form.screenerPreset) ?? screenerPresets[0]
  const scUniverses = screenerUniverses[form.market] ?? []
  const savedUni = form.screenerUniverse?.[form.market]
  const scUniverse =
    (savedUni && (scUniverses.length === 0 || scUniverses.some((u) => u.id === savedUni)) ? savedUni : scUniverses[0]?.id) ??
    (form.market === 'KR' ? 'kr_all_150' : 'us_sp100')
  const scCustom = form.screenerCustom?.[form.market] ?? ''

  const pModel = portfolioModels.find((m) => m.id === form.portfolioModel) ?? portfolioModels[0]
  const pMid = pModel?.id ?? ''
  const pKey = `${pMid}:${form.market}`
  const pAssets: PortfolioAsset[] = form.portfolioAssets[pKey] ?? pModel?.presets[form.market] ?? []
  const pAllowedRoles: PortfolioRole[] = pModel?.allowed_roles ?? ['offensive', 'defensive']
  const pParams = useMemo(() => {
    const saved = form.portfolioParams[pMid] ?? {}
    return Object.fromEntries((pModel?.params ?? []).map((p) => [p.key, saved[p.key] ?? p.default]))
  }, [pModel, pMid, form.portfolioParams])
  const setPAssets = (next: PortfolioAsset[]) =>
    setForm((f) => ({ ...f, portfolioAssets: { ...f.portfolioAssets, [pKey]: next } }))
  const setPParam = (key: string, v: number) =>
    setForm((f) => ({ ...f, portfolioParams: { ...f.portfolioParams, [pMid]: { ...pParams, [key]: v } } }))

  const setParam = (key: string, v: number) =>
    setForm((f) => ({ ...f, params: { ...f.params, [sid]: { ...params, [key]: v } } }))
  const setRange = (p: StrategyParam, field: 'start' | 'stop' | 'step', v: number) =>
    setForm((f) => ({ ...f, ranges: { ...f.ranges, [sid]: { ...(f.ranges[sid] ?? {}), [p.key]: { ...rangeOf(p), [field]: v } } } }))

  const base = (overrideParams?: Record<string, number>, overrideSymbol?: string, overrideStrategy?: string): BacktestRequest | null => {
    const stratId = overrideStrategy ?? meta?.id
    if (!stratId || !costs) return null
    const stratMeta = strategies.find((s) => s.id === stratId) ?? meta
    const stratParams = overrideParams ?? (overrideStrategy && stratMeta
      ? Object.fromEntries(stratMeta.params.map((p) => [p.key, (form.params[stratId] ?? {})[p.key] ?? p.default]))
      : params)
    return {
      market: form.market,
      symbol: (overrideSymbol ?? form.symbol).trim().toUpperCase(),
      strategy: stratId,
      params: stratParams,
      risk: {
        stop_loss_pct: form.stopLossPct || 0,
        take_profit_pct: form.takeProfitPct || 0,
        trailing_stop_pct: form.trailingStopPct || 0,
      },
      market_timing: {
        mode: form.mtMode,
        benchmark: form.mtBenchmark?.[form.market] ?? (form.market === 'KR' ? '069500' : 'SPY'),
        ma_short: 20,
        ma_long: 60,
      },
      period: form.period,
      capital: costs.capital,
      commission_pct: costs.commission_pct,
      tax_pct: costs.tax_pct,
      slippage_pct: costs.slippage_pct,
    }
  }

  const execute = async (mode: Mode, overrideParams?: Record<string, number>, overrideSymbol?: string, overrideStrategy?: string) => {
    if (mode === 'screener') {
      setRunning(true)
      setError('')
      try {
        const r = await quantApi.screenerRun({
          market: form.market,
          preset: scPreset?.id ?? form.screenerPreset,
          universe: scUniverse,
          symbols: scUniverse === 'custom' ? scCustom : undefined,
          strategy: scPreset?.uses_strategy ? form.screenerStrategy : undefined,
          exclude_warnings: form.screenerExcludeWarnings,
          strict_only: form.screenerStrictOnly,
          top_n: form.screenerTopN,
        })
        setResults((s) => ({ ...s, screener: r }))
        if (isMobile) setFormOpen(false)
      } catch (err) {
        setError((err as Error).message)
      } finally {
        setRunning(false)
      }
      return
    }
    if (mode === 'portfolio') {
      if (!pModel || !costs) return
      setRunning(true)
      setError('')
      try {
        const r = await quantApi.portfolioBacktest({
          market: form.market,
          model: pModel.id,
          assets: pAssets,
          params: pParams,
          rebalance: form.portfolioRebalance,
          drift_pct: form.portfolioDriftPct,
          buy_only: form.portfolioBuyOnly,
          period: form.period,
          capital: costs.capital,
          commission_pct: costs.commission_pct,
          tax_pct: costs.tax_pct,
          slippage_pct: costs.slippage_pct,
        })
        setResults((s) => ({ ...s, portfolio: r }))
        if (isMobile) setFormOpen(false)
      } catch (err) {
        setError((err as Error).message)
      } finally {
        setRunning(false)
      }
      return
    }
    const req = base(overrideParams, overrideSymbol, overrideStrategy)
    if (!req || !meta) return
    setRunning(true)
    setError('')
    try {
      const grid = () => {
        const x = meta.params.find((p) => p.key === xKey)!
        const y = meta.params.find((p) => p.key === yKey)
        return { x: rangeOf(x), ...(y ? { y: rangeOf(y) } : {}), objective: form.objective, min_trades: form.minTrades }
      }
      if (mode === 'backtest') {
        const r = await quantApi.backtest(req)
        setResults((s) => ({ ...s, backtest: r }))
      } else if (mode === 'optimize') {
        const r = await quantApi.optimize({ ...req, ...grid() })
        setResults((s) => ({ ...s, optimize: r }))
      } else if (mode === 'walkforward') {
        const r = await quantApi.walkforward({ ...req, ...grid(), is_years: form.isYears, oos_months: form.oosMonths, mode: form.wfMode })
        setResults((s) => ({ ...s, walkforward: r }))
      } else {
        const r = await quantApi.montecarlo({ ...req, runs: form.runs, method: form.mcMethod })
        setResults((s) => ({ ...s, montecarlo: r }))
      }
      if (isMobile) setFormOpen(false)
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setRunning(false)
    }
  }

  const submit = (e: FormEvent) => {
    e.preventDefault()
    execute(form.mode)
  }

  // From the heatmap: adopt that parameter set and run it as a full backtest.
  const pickParams = (p: Record<string, number>) => {
    setForm((f) => ({ ...f, mode: 'backtest', params: { ...f.params, [sid]: { ...params, ...p } } }))
    execute('backtest', { ...params, ...p })
  }

  // From the screener: jump straight to backtest or portfolio mode with extracted picks.
  const backtestFromScreener = (sym: string, stratId: string) => {
    setForm((f) => ({ ...f, mode: 'backtest', symbol: sym, strategy: stratId }))
    execute('backtest', undefined, sym, stratId)
  }
  const portfolioFromScreener = (assets: PortfolioAsset[], modelId: string) => {
    const targetModel = portfolioModels.find((m) => m.id === modelId) ?? portfolioModels[0]
    const mid = targetModel?.id ?? modelId
    const nextKey = `${mid}:${form.market}`
    setForm((f) => ({
      ...f,
      mode: 'portfolio',
      portfolioModel: mid,
      portfolioRebalance: targetModel?.default_rebalance ?? 'monthly',
      portfolioAssets: { ...f.portfolioAssets, [nextKey]: assets },
    }))
  }

  const items = watchlist.filter((w) => w.market === form.market)
  const symbolName = (sym: string) => items.find((w) => w.symbol === sym)?.name ?? sym
  const titleOf = (r: { symbol: string; strategy: string }) =>
    `${symbolName(r.symbol)} (${r.symbol}) · ${strategies.find((s) => s.id === r.strategy)?.name ?? r.strategy}`
  const num = (label: string, value: number, onChange: (v: number) => void, step: number | string = 'any', hint?: string, tip?: string) => (
    <label title={tip}>
      <span>
        {label} {hint && <span className="muted small">{hint}</span>}
      </span>
      <input type="number" step={step} value={value} title={tip} onChange={(e) => onChange(Number(e.target.value))} />
    </label>
  )
  const current = results[form.mode]

  return (
    <div className="quant">
      <form className={`quant-form${isMobile && !formOpen ? ' collapsed' : ''}`} onSubmit={submit}>
        <h2>퀀트 랩</h2>
        {isMobile && (
          <button type="button" className="form-toggle" title="모바일 화면에서 설정 입력창을 접거나 다시 펼칩니다" onClick={() => setFormOpen((o) => !o)}>
            {formOpen ? '설정 접기 ▲' : `설정 열기 ▼ · ${MODES.find((m) => m.id === form.mode)?.label} · ${form.symbol} · ${meta?.name ?? ''}`}
          </button>
        )}
        <div className="mode-tabs">
          {MODES.map((m) => (
            <button type="button" key={m.id} className={m.id === form.mode ? 'active' : ''} title={m.tip} onClick={() => set('mode', m.id)}>
              {m.label}
            </button>
          ))}
        </div>
        <p className="muted small">{MODES.find((m) => m.id === form.mode)?.hint}</p>

        <div className="tabs">
          {(['KR', 'US'] as Market[]).map((m) => (
            <button
              type="button"
              key={m}
              className={m === form.market ? 'active' : ''}
              title={m === 'KR' ? '국내(코스피·코스닥) 주식과 ETF로 실험합니다' : '미국(나스닥·뉴욕) 주식과 ETF로 실험합니다'}
              onClick={() =>
                setForm((f) => ({
                  ...f,
                  market: m,
                  symbol: watchlist.find((w) => w.market === m)?.symbol ?? f.symbol,
                  screenerPreset: m !== 'KR' && f.screenerPreset === 'smart_money' ? 'momentum_quality' : f.screenerPreset,
                }))
              }
            >
              {m === 'KR' ? '국내' : '미국'}
            </button>
          ))}
        </div>

        {!isPortfolio && !isScreener && (
          <>
            <label title="실험할 종목 코드(예: 005930, AAPL)를 직접 입력하거나 관심종목 목록에서 선택합니다">
              <span>종목 코드</span>
              <input
                list="quant-watchlist"
                value={form.symbol}
                title="실험할 종목 코드(예: 005930, AAPL)를 직접 입력하거나 관심종목 목록에서 선택합니다"
                onChange={(e) => set('symbol', e.target.value.toUpperCase())}
              />
              <datalist id="quant-watchlist">
                {items.map((w) => (
                  <option key={w.symbol} value={w.symbol}>{w.name}</option>
                ))}
              </datalist>
              <span className="muted small">{items.find((w) => w.symbol === form.symbol)?.name ?? '관심종목에서 고르거나 아래에서 이름으로 검색'}</span>
            </label>
            <div className="quant-search">
              <input
                type="search"
                value={symQuery}
                placeholder={form.market === 'KR' ? '종목 이름으로 검색 (예: 삼성전자, KODEX)' : '종목·영문명 검색 (예: Apple, NVIDIA)'}
                title="종목 코드를 모를 때 한글 이름이나 영문 티커로 검색해 바로 선택합니다"
                onChange={(e) => setSymQuery(e.target.value)}
              />
              {symHits.length > 0 && (
                <ul className="search-results">
                  {symHits.map((it) => (
                    <li key={it.symbol}>
                      <button
                        type="button"
                        title={`${it.name} (${it.symbol}) 종목을 선택합니다`}
                        onClick={() => {
                          set('symbol', it.symbol)
                          setSymQuery('')
                          setSymHits([])
                        }}
                      >
                        <b>{it.name}</b> <span className="muted small">{it.symbol}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>

            <label title="검증할 투자 전략(일봉 스윙 또는 장중 분봉 단타)을 선택합니다">
              <span>전략</span>
              <select value={sid} title="검증할 투자 전략(일봉 스윙 또는 장중 분봉 단타)을 선택합니다" onChange={(e) => set('strategy', e.target.value)}>
                <optgroup label="스윙 전략 (일봉 · 추세/평균회귀/모멘텀)">
                  {strategies.filter((s) => s.category !== 'daytrade').map((s) => (
                    <option key={s.id} value={s.id}>{s.name}</option>
                  ))}
                </optgroup>
                <optgroup label="단타 전략 (장중·분봉 · 당일 청산)">
                  {strategies.filter((s) => s.category === 'daytrade').map((s) => (
                    <option key={s.id} value={s.id}>{s.name} ({s.timeframe})</option>
                  ))}
                </optgroup>
              </select>
            </label>
            {meta && (
              <>
                <p className="muted small strategy-desc">
                  {meta.theory && <><b>[{meta.category === 'daytrade' ? '단타' : '스윙'} · {meta.timeframe}]</b> {meta.theory}<br /></>}
                  {meta.description}
                </p>
                <StrategyGuideBox theory={meta.theory} guide={meta.guide} />
              </>
            )}
          </>
        )}

        {isPortfolio && pModel && (
          <>
            <label title="여러 자산의 비중을 어떻게 나눌지 정하는 금융 자산배분 이론을 선택합니다">
              <span>자산배분 이론</span>
              <select
                value={pMid}
                title="여러 자산의 비중을 어떻게 나눌지 정하는 금융 자산배분 이론을 선택합니다"
                onChange={(e) => {
                  const next = portfolioModels.find((m) => m.id === e.target.value)
                  setForm((f) => ({ ...f, portfolioModel: e.target.value, portfolioRebalance: next?.default_rebalance ?? f.portfolioRebalance }))
                }}
              >
                {portfolioModels.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
              </select>
            </label>
            <p className="muted small strategy-desc">
              <b>[리밸런싱]</b> {pModel.theory}<br />{pModel.description}
            </p>
            <StrategyGuideBox theory={pModel.theory} guide={pModel.guide} />
            <fieldset className="costs">
              <legend>리밸런싱 규칙</legend>
              <label title="시간이 지나 목표 비중이 흐트러졌을 때 얼마나 자주 다시 맞춰줄지 선택합니다">
                <span>주기 / 조건</span>
                <select
                  value={form.portfolioRebalance}
                  title="시간이 지나 목표 비중이 흐트러졌을 때 얼마나 자주 다시 맞춰줄지 선택합니다"
                  onChange={(e) => set('portfolioRebalance', e.target.value as RebalanceMode)}
                >
                  {Object.entries(rebalModes).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
                </select>
              </label>
              {form.portfolioRebalance === 'drift' && num('허용 편차 (%)', form.portfolioDriftPct, (v) => set('portfolioDriftPct', v), 0.5, undefined, '목표 비중에서 몇 %p 이상 벗어났을 때만 사고팔아 비중을 다시 맞출지 정합니다 (기본 5%)')}
              <label className="check small" title="켜면 기존 보유 주식을 절대 팔지 않고(매도세금·수수료 0원), 남아 있는 현금으로 비중이 부족한 자산만 사서 채웁니다">
                <input type="checkbox" checked={!!form.portfolioBuyOnly} onChange={(e) => set('portfolioBuyOnly', e.target.checked)} />
                <span>무매도(Buy-Only) 리밸런싱 (세금·매도수수료 절감)</span>
              </label>
              {pModel.params.map((p) => {
                const tip = getParamHint(pMid, p.key, p.label, p.min, p.max)
                return (
                  <label className="param" key={p.key} title={tip}>
                    <span>{p.label}</span>
                    <input type="number" min={p.min} max={p.max} step={p.step} value={pParams[p.key]} title={tip} onChange={(e) => setPParam(p.key, Number(e.target.value))} />
                  </label>
                )
              })}
            </fieldset>
            <fieldset className="costs">
              <legend>편입 자산 ({pAssets.length})</legend>
              <div className="asset-editor">
                {pAssets.map((a, idx) => (
                  <div key={idx} className="asset-row">
                    <input value={a.symbol} placeholder={form.market === 'KR' ? '069500' : 'SPY'}
                           title="포트폴리오에 담을 주식·ETF 종목 코드를 입력합니다 (예: 069500, SPY)"
                           onChange={(e) => setPAssets(pAssets.map((x, i) => (i === idx ? { ...x, symbol: e.target.value.toUpperCase() } : x)))} />
                    {pModel.uses_roles && (
                      <select
                        value={a.role}
                        title="이 종목의 역할을 지정합니다 (공격: 수익 추구 자산 / 방어: 하락장 대피 안전자산 / 감시병: 시장 위험 감지기 / 고정: 상시 보유)"
                        onChange={(e) => setPAssets(pAssets.map((x, i) => (i === idx ? { ...x, role: e.target.value as PortfolioRole } : x)))}
                      >
                        {pAllowedRoles.map((role) => (
                          <option key={role} value={role}>{ROLE_LABELS[role]}</option>
                        ))}
                      </select>
                    )}
                    {pModel.uses_weights && (
                      <input type="number" min={0} max={100} step={0.5} value={a.weight} title="전체 투자금 중 이 종목을 몇 % 담을지 목표 비중을 입력합니다"
                             onChange={(e) => setPAssets(pAssets.map((x, i) => (i === idx ? { ...x, weight: Number(e.target.value) } : x)))} />
                    )}
                    <button type="button" className="link danger-link" disabled={pAssets.length <= 2}
                            title="이 종목을 포트폴리오에서 뺍니다 (최소 2종목 필요)"
                            onClick={() => setPAssets(pAssets.filter((_, i) => i !== idx))}>×</button>
                  </div>
                ))}
                <div className="tabs">
                  <button type="button" disabled={pAssets.length >= 16}
                          title="포트폴리오에 새 종목 입력 칸을 추가합니다 (최대 16종목)"
                          onClick={() => setPAssets([...pAssets, { symbol: '', role: pAllowedRoles[0] ?? 'offensive', weight: 10 }])}>+ 종목 추가</button>
                  <button type="button" title="이 자산배분 이론의 표준 추천 ETF 구성으로 되돌립니다" onClick={() => setPAssets(pModel.presets[form.market] ?? [])}>기본 프리셋</button>
                </div>
              </div>
            </fieldset>
          </>
        )}

        {isScreener && scPreset && (
          <>
            <label title="어떤 기준(추세·눌림목·수급·저평가·배당·전략궁합)으로 유망 종목을 골라낼지 선택합니다">
              <span>종목 발굴 방식</span>
              <select value={scPreset.id} title="어떤 기준으로 유망 종목을 골라낼지 선택합니다" onChange={(e) => set('screenerPreset', e.target.value)}>
                {screenerPresets.map((p) => (
                  <option key={p.id} value={p.id} disabled={p.kr_only && form.market !== 'KR'}>
                    {p.name}{p.kr_only && form.market !== 'KR' ? ' (국내 전용)' : ''}
                  </option>
                ))}
              </select>
            </label>
            <p className="muted small strategy-desc">
              <b>[종목 발굴]</b> {scPreset.theory}<br />{scPreset.description}
            </p>
            <StrategyGuideBox theory={scPreset.theory} guide={scPreset.guide} />

            <label title="어느 시장·대표 종목군 안에서 종목을 스캔할지 범위를 선택합니다">
              <span>후보 종목 묶음</span>
              <select
                value={scUniverse}
                title="어느 시장·대표 종목군 안에서 종목을 스캔할지 범위를 선택합니다"
                onChange={(e) => setForm((f) => ({ ...f, screenerUniverse: { ...f.screenerUniverse, [f.market]: e.target.value } }))}
              >
                {scUniverses.map((u) => (
                  <option key={u.id} value={u.id}>{u.name}</option>
                ))}
              </select>
              <span className="muted small">{scUniverses.find((u) => u.id === scUniverse)?.description}</span>
            </label>

            {scUniverse === 'custom' && (
              <label title="직접 비교하고 싶은 종목 코드들을 쉼표(,)로 구분해 최대 30개까지 입력합니다">
                <span>직접 입력 종목 코드 (쉼표 구분, 최대 30개)</span>
                <textarea
                  rows={3}
                  value={scCustom}
                  placeholder={form.market === 'KR' ? '005930, 000660, 005380' : 'AAPL, MSFT, NVDA, GOOGL'}
                  title="직접 비교하고 싶은 종목 코드들을 쉼표(,)로 구분해 최대 30개까지 입력합니다"
                  onChange={(e) => setForm((f) => ({ ...f, screenerCustom: { ...f.screenerCustom, [f.market]: e.target.value.toUpperCase() } }))}
                />
              </label>
            )}

            {scPreset.uses_strategy && (
              <label title="각 후보 종목에 실제로 대입해 최근 2년 수익률·승률·낙폭 궁합을 채점할 전략을 고릅니다">
                <span>궁합을 테스트할 전략 (일봉)</span>
                <select value={form.screenerStrategy} title="각 후보 종목에 대입해 궁합을 채점할 일봉 전략을 고릅니다" onChange={(e) => set('screenerStrategy', e.target.value)}>
                  {dailyStrategies.map((s) => (
                    <option key={s.id} value={s.id}>{s.name}</option>
                  ))}
                </select>
              </label>
            )}

            <fieldset className="costs">
              <legend>발굴 옵션</legend>
              {num('상위 표시 종목 수', form.screenerTopN, (v) => set('screenerTopN', Math.max(1, Math.min(40, v))), 1, undefined, '종합 점수가 높은 상위 몇 종목까지 결과 표에 보여줄지 정합니다 (1~40개)')}
              <label className="check" title="켜면 선택한 발굴 방식의 핵심 기준(★ 조건 충족)을 엄격하게 통과한 종목만 추려서 보여줍니다">
                <input
                  type="checkbox"
                  checked={form.screenerStrictOnly}
                  onChange={(e) => set('screenerStrictOnly', e.target.checked)}
                />
                <span>조건 완벽 충족 종목만 보기 (★ 필터)</span>
              </label>
              <label className="check" title="투자경고·단기과열·관리종목·정리매매 등 거래소 위험 지정 종목을 자동으로 목록에서 빼줍니다">
                <input
                  type="checkbox"
                  checked={form.screenerExcludeWarnings}
                  onChange={(e) => set('screenerExcludeWarnings', e.target.checked)}
                />
                <span>투자경고·단기과열·정리매매 종목 자동 제외</span>
              </label>
            </fieldset>
          </>
        )}

        {meta && !gridMode && !isPortfolio && !isScreener && (
          <>
            <div className="params">
              {meta.params.map((p) => {
                const tip = getParamHint(sid, p.key, p.label, p.min, p.max)
                return (
                  <label className="param" key={p.key} title={tip}>
                    <span>{p.label}</span>
                    <input type="number" min={p.min} max={p.max} step={p.step} value={params[p.key]} title={tip} onChange={(e) => setParam(p.key, Number(e.target.value))} />
                  </label>
                )
              })}
            </div>
            <fieldset className="costs">
              <legend>공통 보호막 · 익절/손절 (0이면 끔)</legend>
              {num('공통 손절 (%)', form.stopLossPct, (v) => set('stopLossPct', Math.max(0, Math.min(80, v))), 0.5, undefined, '매수가 대비 이 비율(%)만큼 떨어지면 전략 신호를 기다리지 않고 즉시 손절합니다 (0이면 사용 안 함)')}
              {num('공통 목표 익절 (%)', form.takeProfitPct, (v) => set('takeProfitPct', Math.max(0, Math.min(80, v))), 0.5, undefined, '매수가 대비 이 비율(%)만큼 오르면 목표 수익을 확정하고 즉시 익절합니다 (0이면 사용 안 함)')}
              {num('트레일링 스톱 (%)', form.trailingStopPct, (v) => set('trailingStopPct', Math.max(0, Math.min(80, v))), 0.5, undefined, '매수 후 찍은 최고가 대비 이 비율(%)만큼 되밀리면 수익을 지키며 자동 매도합니다 (0이면 사용 안 함)')}
            </fieldset>
            <fieldset className="costs">
              <legend>🚦 마켓 타이밍 신호등 필터</legend>
              <label title="코스피·S&P500 대표 지수가 20일·60일 평균선 아래로 무너진 하락장일 때 어떻게 방어할지 고릅니다">
                <span>하락장 대응 방식</span>
                <select value={form.mtMode} title="시장 전체가 하락장일 때 대응 방식을 선택합니다" onChange={(e) => set('mtMode', e.target.value as MarketTimingMode)}>
                  <option value="off">꺼짐 (항상 전략대로 매매)</option>
                  <option value="block_buy">하락장(🔴)에 신규 매수 중단 (권장)</option>
                  <option value="half_size">하락장(🔴)에 매수 비중 50% 축소</option>
                  <option value="exit_all">하락장(🔴) 진입 시 보유 주식도 즉시 현금화</option>
                </select>
              </label>
              {form.mtMode !== 'off' && (
                <label title="시장 온도를 측정할 대표 지수 ETF를 고릅니다">
                  <span>기준 시장 지수</span>
                  <select
                    value={form.mtBenchmark?.[form.market] ?? (form.market === 'KR' ? '069500' : 'SPY')}
                    title="시장 온도를 측정할 대표 지수 ETF를 선택합니다"
                    onChange={(e) => setForm((f) => ({ ...f, mtBenchmark: { ...(f.mtBenchmark ?? { KR: '069500', US: 'SPY' }), [f.market]: e.target.value } }))}
                  >
                    {form.market === 'KR' ? (
                      <>
                        <option value="069500">KODEX 200 (코스피 대표)</option>
                        <option value="229200">KODEX 코스닥150 (코스닥 대표)</option>
                      </>
                    ) : (
                      <>
                        <option value="SPY">SPY (미국 S&P500 대표)</option>
                        <option value="QQQ">QQQ (미국 나스닥100 대표)</option>
                      </>
                    )}
                  </select>
                </label>
              )}
            </fieldset>
          </>
        )}

        {meta && gridMode && (
          <fieldset className="costs">
            <legend>탐색 범위</legend>
            <label title="가로축(X축)에 놓고 숫자를 조금씩 바꿔가며 비교할 첫 번째 설정 항목을 고릅니다">
              <span>X축 파라미터</span>
              <select
                value={xKey}
                title="가로축(X축)에 놓고 숫자를 바꿔가며 비교할 첫 번째 설정 항목을 고릅니다"
                onChange={(e) => setForm((f) => ({ ...f, xKey: { ...f.xKey, [sid]: e.target.value },
                  yKey: { ...f.yKey, [sid]: (f.yKey[sid] ?? yKey) === e.target.value ? '' : (f.yKey[sid] ?? yKey) } }))}
              >
                {meta.params.map((p) => <option key={p.key} value={p.key}>{p.label}</option>)}
              </select>
            </label>
            <label title="세로축(Y축)에 놓고 함께 비교할 두 번째 설정 항목을 고릅니다 ('없음'을 고르면 1개만 집중 탐색)">
              <span>Y축 파라미터</span>
              <select
                value={yKey}
                title="세로축(Y축)에 놓고 함께 비교할 두 번째 설정 항목을 고릅니다"
                onChange={(e) => setForm((f) => ({ ...f, yKey: { ...f.yKey, [sid]: e.target.value } }))}
              >
                <option value="">없음 (1개만 탐색)</option>
                {meta.params.filter((p) => p.key !== xKey).map((p) => <option key={p.key} value={p.key}>{p.label}</option>)}
              </select>
            </label>
            {meta.params.map((p) => {
              const onAxis = p.key === xKey || p.key === yKey
              const r = rangeOf(p)
              const baseTip = getParamHint(sid, p.key, p.label, p.min, p.max)
              return onAxis ? (
                <div key={p.key} className="range-row">
                  <span className="small" title={baseTip}>{p.label} <span className="muted">({p.key === xKey ? 'X' : 'Y'})</span></span>
                  <div className="range-inputs">
                    {(['start', 'stop', 'step'] as const).map((f) => {
                      const fTip = f === 'start'
                        ? `${p.label} 탐색을 시작할 가장 작은 숫자입니다`
                        : f === 'stop'
                          ? `${p.label} 탐색을 끝낼 가장 큰 숫자입니다`
                          : `${p.label} 숫자를 한 칸당 얼마씩 늘려가며 비교할지 간격입니다`
                      return (
                        <label key={f} className="param" title={fTip}>
                          <span>{f === 'start' ? '시작' : f === 'stop' ? '끝' : '간격'}</span>
                          <input type="number" step={p.step} value={r[f]} title={fTip} onChange={(e) => setRange(p, f, Number(e.target.value))} />
                        </label>
                      )
                    })}
                  </div>
                </div>
              ) : (
                <label key={p.key} className="param fixed" title={baseTip}>
                  <span>{p.label} (고정)</span>
                  <input type="number" min={p.min} max={p.max} step={p.step} value={params[p.key]} title={baseTip} onChange={(e) => setParam(p.key, Number(e.target.value))} />
                </label>
              )
            })}
            <label title="여러 설정 조합 중 어떤 성적이 가장 좋은 조합을 1등으로 뽑을지 채점 기준을 고릅니다 (위험 대비 수익인 샤프 지수 권장)">
              <span>목표 지표 (무엇을 최대화할지)</span>
              <select value={form.objective} title="여러 설정 조합 중 무엇을 최우선으로 채점할지 선택합니다" onChange={(e) => set('objective', e.target.value)}>
                {Object.entries(objectives).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
              </select>
            </label>
            {num('최소 매매 횟수', form.minTrades, (v) => set('minTrades', v), 1, '· 미만이면 최고값 후보 제외', '전체 기간 동안 매매 횟수가 이 숫자보다 적으면 우연한 결과로 보고 추천 후보에서 제외합니다')}
          </fieldset>
        )}

        {form.mode === 'walkforward' && (
          <fieldset className="costs">
            <legend>구간 설정</legend>
            {num('학습 기간 (년)', form.isYears, (v) => set('isYears', v), 0.5, undefined, '과거 몇 년치 데이터로 가장 좋은 파라미터를 공부(탐색)할지 정합니다 (보통 2~4년)')}
            {num('검증 기간 (개월)', form.oosMonths, (v) => set('oosMonths', v), 1, undefined, '공부한 설정을 처음 보는 다음 몇 개월 동안 실전처럼 시험해 볼지 정합니다 (보통 6~12개월)')}
            <div className="tabs">
              {(['rolling', 'anchored'] as const).map((m) => (
                <button type="button" key={m} className={form.wfMode === m ? 'active' : ''} onClick={() => set('wfMode', m)}
                        title={m === 'rolling' ? '롤링 방식: 학습 기간 길이를 일정하게 유지하며 창문처럼 옆으로 이동합니다 (최근 시장 성격 반영에 유리)' : '누적 방식: 시작 시점을 고정하고 학습 데이터를 계속 쌓아가며 검증합니다'}>
                  {m === 'rolling' ? '롤링' : '누적'}
                </button>
              ))}
            </div>
          </fieldset>
        )}

        {form.mode === 'montecarlo' && (
          <fieldset className="costs">
            <legend>시뮬레이션</legend>
            {num('반복 횟수', form.runs, (v) => set('runs', v), 100, undefined, '무작위 가상 시나리오를 몇 번 반복해 뽑아볼지 정합니다 (1,000회 이상 권장)')}
            <div className="tabs">
              {(['bootstrap', 'shuffle'] as const).map((m) => (
                <button type="button" key={m} className={form.mcMethod === m ? 'active' : ''} onClick={() => set('mcMethod', m)}
                        title={m === 'bootstrap' ? '부트스트랩: 과거 매매들을 중복 허용으로 다시 뽑아 대박이 빠지거나 연패가 겹치는 수천 가지 가상 미래를 만듭니다' : '순서 섞기: 실제 매매들의 순서만 무작위로 바꿔 연패가 한꺼번에 몰렸을 때 계좌 낙폭이 얼마나 커지는지 확인합니다'}>
                  {m === 'bootstrap' ? '부트스트랩' : '순서 섞기'}
                </button>
              ))}
            </div>
          </fieldset>
        )}

        {!isScreener && (
          <>
            {isIntraday ? (
              <p className="muted small">
                <b>분봉 데이터 ({meta?.timeframe})</b>: 최근 약 60거래일 분봉을 자동 조회하며, 장 마감 직전에 당일 전량 청산합니다.
              </p>
            ) : (
              <label>
                <span>기간 (일봉)</span>
                <div className="tabs">
                  {PERIODS.map((p) => (
                    <button
                      type="button"
                      key={p.id}
                      className={p.id === form.period ? 'active' : ''}
                      title={p.id === 'max' ? '조회 가능한 전체 과거 일봉 기간으로 검증합니다' : `최근 ${p.label} 동안의 과거 일봉 데이터로 검증합니다`}
                      onClick={() => set('period', p.id)}
                    >
                      {p.label}
                    </button>
                  ))}
                </div>
              </label>
            )}

            {wfBlockedByIntraday && (
              <p className="warn small">
                워크포워드는 수년 단위 검증이므로 일봉 전략에서만 사용할 수 있습니다. 분봉 단타 전략은 [백테스트 · 최적화 · 몬테카를로] 탭을 이용해 주세요.
              </p>
            )}

            {costs && (
              <fieldset className="costs">
                <legend>자본 · 비용</legend>
                {num(`초기 자본 (${costs.currency})`, costs.capital, (v) => setCosts({ ...costs, capital: v }), 1, undefined, '시뮬레이션을 시작할 초기 가상 투자 원금입니다')}
                {num('수수료 % (매수·매도 각각)', costs.commission_pct, (v) => setCosts({ ...costs, commission_pct: v }), 0.001,
                  costs.commission_source === 'toss' ? '· 내 토스 계좌 수수료' : '· 기본값',
                  '주식을 살 때와 팔 때 각각 빠져나가는 증권사 매매 수수료율(%)입니다')}
                <label title="주식을 팔 때 부과되는 증권거래세율(%)입니다 (국내 일반주식은 기본 세율 적용, 국내 상장 ETF와 미국 주식은 0%)">
                  <span>
                    매도 세금 %{' '}
                    {form.market === 'KR' && (
                      <span className="muted small">
                        {isPortfolio ? '· 국내 ETF는 증권거래세 0% 면제 ' : '· 증권거래세, ETF는 0% '}
                        {costs.tax_pct > 0 && (
                          <button
                            type="button"
                            className="link"
                            title="국내 상장 ETF는 매도 시 증권거래세가 면제되므로 세금을 0%로 바로 바꿉니다"
                            onClick={() => setCosts({ ...costs, tax_pct: 0 })}
                          >
                            [0% 적용]
                          </button>
                        )}
                      </span>
                    )}
                  </span>
                  <input
                    type="number"
                    step={0.01}
                    value={costs.tax_pct}
                    title="주식을 팔 때 부과되는 증권거래세율(%)입니다 (국내 ETF·미국 주식은 0%)"
                    onChange={(e) => setCosts({ ...costs, tax_pct: Number(e.target.value) })}
                  />
                </label>
                {num('슬리피지 % (체결마다)', costs.slippage_pct, (v) => setCosts({ ...costs, slippage_pct: v }), 0.01, undefined, '주문 순간 호가가 한두 칸 밀려 생각보다 조금 비싸게 사거나 싸게 팔리는 현실적인 체결 오차 비용(%)입니다')}
              </fieldset>
            )}
          </>
        )}

        <button
          type="submit"
          title={wfBlockedByIntraday ? '워크포워드 검증은 수년치 일봉 데이터를 사용하므로 일봉 스윙 전략에서만 실행할 수 있습니다' : `왼쪽에서 설정한 조건으로 ${MODES.find((m) => m.id === form.mode)?.label ?? '시뮬레이션'} 계산을 시작합니다`}
          disabled={
            running ||
            wfBlockedByIntraday ||
            (isScreener
              ? !scPreset || (scPreset.kr_only && form.market !== 'KR')
              : !costs || (isPortfolio ? (!pModel || pAssets.length < 2) : (!meta || !form.symbol.trim())))
          }
        >
          {running ? (isScreener ? '종목 스캔 중…' : '계산 중…') : `${MODES.find((m) => m.id === form.mode)?.label} 실행`}
        </button>
        {error && <p className="error">{error}</p>}
        <p className="muted small">
          {isScreener
            ? '추출 결과는 과거 가격·수급·재무 데이터를 바탕으로 한 참고 점수입니다. 최종 매수 전 최신 뉴스와 백테스트를 함께 확인하세요.'
            : '신호는 그날 종가로 판단하고 다음 날 체결합니다 (시장가는 다음 날 시가, 돌파 주문은 장중 기준가). 과거 성과가 미래 수익을 보장하지 않습니다.'}
        </p>
      </form>

      <section className="quant-results">
        <FeatureGuideBox guide={FEATURE_GUIDES.quant} />
        {!current && <div className="placeholder muted">왼쪽에서 설정하고 실행하세요.</div>}
        {form.mode === 'backtest' && results.backtest && <BacktestView result={results.backtest} title={titleOf(results.backtest)} />}
        {form.mode === 'optimize' && results.optimize && (
          <OptimizeView result={results.optimize} title={titleOf(results.optimize)}
                        objectiveLabel={objectives[results.optimize.objective] ?? results.optimize.objective} onPick={pickParams} />
        )}
        {form.mode === 'walkforward' && results.walkforward && (
          <WalkForwardView result={results.walkforward} title={titleOf(results.walkforward)}
                           objectiveLabel={objectives[results.walkforward.objective] ?? results.walkforward.objective} onPick={pickParams} />
        )}
        {form.mode === 'montecarlo' && results.montecarlo && <MonteCarloView result={results.montecarlo} title={titleOf(results.montecarlo)} />}
        {form.mode === 'portfolio' && results.portfolio && (
          <PortfolioView result={results.portfolio} model={portfolioModels.find((m) => m.id === results.portfolio?.model)} />
        )}
        {form.mode === 'screener' && results.screener && (
          <ScreenerView
            result={results.screener}
            preset={screenerPresets.find((p) => p.id === results.screener?.preset)}
            onBacktest={backtestFromScreener}
            onPortfolio={portfolioFromScreener}
          />
        )}
      </section>
    </div>
  )
}
