import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { marketApi } from '../api/market'
import {
  quantApi,
  type AxisRange,
  type BacktestRequest,
  type BacktestResult,
  type CostDefaults,
  type MonteCarloResult,
  type OptimizeResult,
  type Period,
  type PortfolioAsset,
  type PortfolioBacktestResult,
  type PortfolioModelMeta,
  type RebalanceMode,
  type StrategyMeta,
  type StrategyParam,
  type WalkForwardResult,
} from '../api/quant'
import type { Market, WatchItem } from '../api/types'
import { MOBILE, useMediaQuery } from '../hooks/useMediaQuery'
import BacktestView from '../components/quant/BacktestView'
import MonteCarloView from '../components/quant/MonteCarloView'
import OptimizeView from '../components/quant/OptimizeView'
import PortfolioView from '../components/quant/PortfolioView'
import WalkForwardView from '../components/quant/WalkForwardView'

type Mode = 'backtest' | 'optimize' | 'walkforward' | 'montecarlo' | 'portfolio'

const MODES: { id: Mode; label: string; hint: string }[] = [
  { id: 'backtest', label: '백테스트', hint: '설정 하나로 과거 성과 확인' },
  { id: 'optimize', label: '최적화', hint: '파라미터 조합을 격자로 비교' },
  { id: 'walkforward', label: '워크포워드', hint: '처음 보는 기간에도 통하는지 검증' },
  { id: 'montecarlo', label: '몬테카를로', hint: '매매 순서·구성의 운을 제거' },
  { id: 'portfolio', label: '포트폴리오', hint: '다중 ETF 자산배분·리밸런싱 검증' },
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
  portfolioParams: Record<string, Record<string, number>>
  portfolioAssets: Record<string, PortfolioAsset[]>
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
  portfolioParams: {},
  portfolioAssets: {},
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
  const [rebalModes, setRebalModes] = useState<Record<RebalanceMode, string>>({
    monthly: '매월 초', quarterly: '분기 초', yearly: '매년 초', drift: '비중 편차 이탈 시',
  })
  const [objectives, setObjectives] = useState<Record<string, string>>({})
  const [watchlist, setWatchlist] = useState<WatchItem[]>([])
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
    marketApi.watchlist().then(({ items }) => setWatchlist(items)).catch(() => undefined)
  }, [])

  useEffect(() => localStorage.setItem(FORM_KEY, JSON.stringify(form)), [form])

  useEffect(() => {
    setCosts(null)
    quantApi.defaults(form.market).then(setCosts).catch((e) => setError(e.message))
  }, [form.market])

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

  const pModel = portfolioModels.find((m) => m.id === form.portfolioModel) ?? portfolioModels[0]
  const pMid = pModel?.id ?? ''
  const pKey = `${pMid}:${form.market}`
  const pAssets: PortfolioAsset[] = form.portfolioAssets[pKey] ?? pModel?.presets[form.market] ?? []
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

  const base = (overrideParams?: Record<string, number>): BacktestRequest | null =>
    meta && costs
      ? {
          market: form.market,
          symbol: form.symbol.trim().toUpperCase(),
          strategy: meta.id,
          params: overrideParams ?? params,
          period: form.period,
          capital: costs.capital,
          commission_pct: costs.commission_pct,
          tax_pct: costs.tax_pct,
          slippage_pct: costs.slippage_pct,
        }
      : null

  const execute = async (mode: Mode, overrideParams?: Record<string, number>) => {
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
    const req = base(overrideParams)
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

  const items = watchlist.filter((w) => w.market === form.market)
  const symbolName = (sym: string) => items.find((w) => w.symbol === sym)?.name ?? sym
  const titleOf = (r: { symbol: string; strategy: string }) =>
    `${symbolName(r.symbol)} (${r.symbol}) · ${strategies.find((s) => s.id === r.strategy)?.name ?? r.strategy}`
  const num = (label: string, value: number, onChange: (v: number) => void, step: number | string = 'any', hint?: string) => (
    <label>
      <span>
        {label} {hint && <span className="muted small">{hint}</span>}
      </span>
      <input type="number" step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} />
    </label>
  )
  const current = results[form.mode]

  return (
    <div className="quant">
      <form className={`quant-form${isMobile && !formOpen ? ' collapsed' : ''}`} onSubmit={submit}>
        <h2>퀀트 랩</h2>
        {isMobile && (
          <button type="button" className="form-toggle" onClick={() => setFormOpen((o) => !o)}>
            {formOpen ? '설정 접기 ▲' : `설정 열기 ▼ · ${MODES.find((m) => m.id === form.mode)?.label} · ${form.symbol} · ${meta?.name ?? ''}`}
          </button>
        )}
        <div className="mode-tabs">
          {MODES.map((m) => (
            <button type="button" key={m.id} className={m.id === form.mode ? 'active' : ''} title={m.hint} onClick={() => set('mode', m.id)}>
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
              onClick={() => setForm((f) => ({ ...f, market: m, symbol: watchlist.find((w) => w.market === m)?.symbol ?? f.symbol }))}
            >
              {m === 'KR' ? '국내' : '미국'}
            </button>
          ))}
        </div>

        {!isPortfolio && (
          <>
            <label>
              <span>종목</span>
              <input list="quant-watchlist" value={form.symbol} onChange={(e) => set('symbol', e.target.value.toUpperCase())} />
              <datalist id="quant-watchlist">
                {items.map((w) => (
                  <option key={w.symbol} value={w.symbol}>{w.name}</option>
                ))}
              </datalist>
              <span className="muted small">{items.find((w) => w.symbol === form.symbol)?.name ?? '관심종목에서 고르거나 코드를 입력'}</span>
            </label>

            <label>
              <span>전략</span>
              <select value={sid} onChange={(e) => set('strategy', e.target.value)}>
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
              <p className="muted small strategy-desc">
                {meta.theory && <><b>[{meta.category === 'daytrade' ? '단타' : '스윙'} · {meta.timeframe}]</b> {meta.theory}<br /></>}
                {meta.description}
              </p>
            )}
          </>
        )}

        {isPortfolio && pModel && (
          <>
            <label>
              <span>자산배분 이론</span>
              <select value={pMid} onChange={(e) => {
                const next = portfolioModels.find((m) => m.id === e.target.value)
                setForm((f) => ({ ...f, portfolioModel: e.target.value, portfolioRebalance: next?.default_rebalance ?? f.portfolioRebalance }))
              }}>
                {portfolioModels.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
              </select>
            </label>
            <p className="muted small strategy-desc">
              <b>[리밸런싱]</b> {pModel.theory}<br />{pModel.description}
            </p>
            <fieldset className="costs">
              <legend>리밸런싱 규칙</legend>
              <label>
                <span>주기 / 조건</span>
                <select value={form.portfolioRebalance} onChange={(e) => set('portfolioRebalance', e.target.value as RebalanceMode)}>
                  {Object.entries(rebalModes).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
                </select>
              </label>
              {form.portfolioRebalance === 'drift' && num('허용 편차 (%)', form.portfolioDriftPct, (v) => set('portfolioDriftPct', v), 0.5)}
              {pModel.params.map((p) => (
                <label className="param" key={p.key}>
                  <span>{p.label}</span>
                  <input type="number" min={p.min} max={p.max} step={p.step} value={pParams[p.key]} onChange={(e) => setPParam(p.key, Number(e.target.value))} />
                </label>
              ))}
            </fieldset>
            <fieldset className="costs">
              <legend>편입 자산 ({pAssets.length})</legend>
              <div className="asset-editor">
                {pAssets.map((a, idx) => (
                  <div key={idx} className="asset-row">
                    <input value={a.symbol} placeholder={form.market === 'KR' ? '069500' : 'SPY'}
                           onChange={(e) => setPAssets(pAssets.map((x, i) => (i === idx ? { ...x, symbol: e.target.value.toUpperCase() } : x)))} />
                    {pModel.uses_roles && (
                      <select value={a.role} onChange={(e) => setPAssets(pAssets.map((x, i) => (i === idx ? { ...x, role: e.target.value as 'offensive' | 'defensive' } : x)))}>
                        <option value="offensive">공격</option>
                        <option value="defensive">방어</option>
                      </select>
                    )}
                    {pModel.uses_weights && (
                      <input type="number" min={0} max={100} step={0.5} value={a.weight} title="비중 (%)"
                             onChange={(e) => setPAssets(pAssets.map((x, i) => (i === idx ? { ...x, weight: Number(e.target.value) } : x)))} />
                    )}
                    <button type="button" className="link danger-link" disabled={pAssets.length <= 2}
                            onClick={() => setPAssets(pAssets.filter((_, i) => i !== idx))}>×</button>
                  </div>
                ))}
                <div className="tabs">
                  <button type="button" disabled={pAssets.length >= 12}
                          onClick={() => setPAssets([...pAssets, { symbol: '', role: 'offensive', weight: 10 }])}>+ 종목 추가</button>
                  <button type="button" onClick={() => setPAssets(pModel.presets[form.market] ?? [])}>기본 프리셋</button>
                </div>
              </div>
            </fieldset>
          </>
        )}

        {meta && !gridMode && !isPortfolio && (
          <div className="params">
            {meta.params.map((p) => (
              <label className="param" key={p.key}>
                <span>{p.label}</span>
                <input type="number" min={p.min} max={p.max} step={p.step} value={params[p.key]} onChange={(e) => setParam(p.key, Number(e.target.value))} />
              </label>
            ))}
          </div>
        )}

        {meta && gridMode && (
          <fieldset className="costs">
            <legend>탐색 범위</legend>
            <label>
              <span>X축 파라미터</span>
              <select value={xKey} onChange={(e) => setForm((f) => ({ ...f, xKey: { ...f.xKey, [sid]: e.target.value },
                yKey: { ...f.yKey, [sid]: (f.yKey[sid] ?? yKey) === e.target.value ? '' : (f.yKey[sid] ?? yKey) } }))}>
                {meta.params.map((p) => <option key={p.key} value={p.key}>{p.label}</option>)}
              </select>
            </label>
            <label>
              <span>Y축 파라미터</span>
              <select value={yKey} onChange={(e) => setForm((f) => ({ ...f, yKey: { ...f.yKey, [sid]: e.target.value } }))}>
                <option value="">없음 (1개만 탐색)</option>
                {meta.params.filter((p) => p.key !== xKey).map((p) => <option key={p.key} value={p.key}>{p.label}</option>)}
              </select>
            </label>
            {meta.params.map((p) => {
              const onAxis = p.key === xKey || p.key === yKey
              const r = rangeOf(p)
              return onAxis ? (
                <div key={p.key} className="range-row">
                  <span className="small">{p.label} <span className="muted">({p.key === xKey ? 'X' : 'Y'})</span></span>
                  <div className="range-inputs">
                    {(['start', 'stop', 'step'] as const).map((f) => (
                      <label key={f} className="param">
                        <span>{f === 'start' ? '시작' : f === 'stop' ? '끝' : '간격'}</span>
                        <input type="number" step={p.step} value={r[f]} onChange={(e) => setRange(p, f, Number(e.target.value))} />
                      </label>
                    ))}
                  </div>
                </div>
              ) : (
                <label key={p.key} className="param fixed">
                  <span>{p.label} (고정)</span>
                  <input type="number" min={p.min} max={p.max} step={p.step} value={params[p.key]} onChange={(e) => setParam(p.key, Number(e.target.value))} />
                </label>
              )
            })}
            <label>
              <span>목표 지표 (무엇을 최대화할지)</span>
              <select value={form.objective} onChange={(e) => set('objective', e.target.value)}>
                {Object.entries(objectives).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
              </select>
            </label>
            {num('최소 매매 횟수', form.minTrades, (v) => set('minTrades', v), 1, '· 미만이면 최고값 후보 제외')}
          </fieldset>
        )}

        {form.mode === 'walkforward' && (
          <fieldset className="costs">
            <legend>구간 설정</legend>
            {num('학습 기간 (년)', form.isYears, (v) => set('isYears', v), 0.5)}
            {num('검증 기간 (개월)', form.oosMonths, (v) => set('oosMonths', v), 1)}
            <div className="tabs">
              {(['rolling', 'anchored'] as const).map((m) => (
                <button type="button" key={m} className={form.wfMode === m ? 'active' : ''} onClick={() => set('wfMode', m)}
                        title={m === 'rolling' ? '학습 구간이 일정 길이로 이동' : '학습 구간이 처음부터 계속 늘어남'}>
                  {m === 'rolling' ? '롤링' : '누적'}
                </button>
              ))}
            </div>
          </fieldset>
        )}

        {form.mode === 'montecarlo' && (
          <fieldset className="costs">
            <legend>시뮬레이션</legend>
            {num('반복 횟수', form.runs, (v) => set('runs', v), 100)}
            <div className="tabs">
              {(['bootstrap', 'shuffle'] as const).map((m) => (
                <button type="button" key={m} className={form.mcMethod === m ? 'active' : ''} onClick={() => set('mcMethod', m)}
                        title={m === 'bootstrap' ? '매매를 복원 추출해 새 조합 생성' : '실제 매매의 순서만 섞음 (최종 수익 동일, 낙폭 분포 확인)'}>
                  {m === 'bootstrap' ? '부트스트랩' : '순서 섞기'}
                </button>
              ))}
            </div>
          </fieldset>
        )}

        <label>
          <span>기간 (일봉)</span>
          <div className="tabs">
            {PERIODS.map((p) => (
              <button type="button" key={p.id} className={p.id === form.period ? 'active' : ''} onClick={() => set('period', p.id)}>
                {p.label}
              </button>
            ))}
          </div>
        </label>

        {costs && (
          <fieldset className="costs">
            <legend>자본 · 비용</legend>
            {num(`초기 자본 (${costs.currency})`, costs.capital, (v) => setCosts({ ...costs, capital: v }), 1)}
            {num('수수료 % (매수·매도 각각)', costs.commission_pct, (v) => setCosts({ ...costs, commission_pct: v }), 0.001,
              costs.commission_source === 'toss' ? '· 내 토스 계좌 수수료' : '· 기본값')}
            {num('매도 세금 %', costs.tax_pct, (v) => setCosts({ ...costs, tax_pct: v }), 0.01, form.market === 'KR' ? '· 증권거래세, 현재 세율 확인' : '')}
            {num('슬리피지 % (체결마다)', costs.slippage_pct, (v) => setCosts({ ...costs, slippage_pct: v }), 0.01)}
          </fieldset>
        )}

        <button type="submit" disabled={running || !costs || (isPortfolio ? (!pModel || pAssets.length < 2) : (!meta || !form.symbol.trim()))}>
          {running ? '계산 중…' : `${MODES.find((m) => m.id === form.mode)?.label} 실행`}
        </button>
        {error && <p className="error">{error}</p>}
        <p className="muted small">
          신호는 그날 종가로 판단하고 다음 날 체결합니다 (시장가는 다음 날 시가, 돌파 주문은 장중 기준가). 과거 성과가 미래 수익을 보장하지 않습니다.
        </p>
      </form>

      <section className="quant-results">
        {!current && <div className="placeholder muted">왼쪽에서 설정하고 실행하세요.</div>}
        {form.mode === 'backtest' && results.backtest && <BacktestView result={results.backtest} title={titleOf(results.backtest)} />}
        {form.mode === 'optimize' && results.optimize && (
          <OptimizeView result={results.optimize} title={titleOf(results.optimize)}
                        objectiveLabel={objectives[results.optimize.objective] ?? results.optimize.objective} onPick={pickParams} />
        )}
        {form.mode === 'walkforward' && results.walkforward && (
          <WalkForwardView result={results.walkforward} title={titleOf(results.walkforward)}
                           objectiveLabel={objectives[results.walkforward.objective] ?? results.walkforward.objective} />
        )}
        {form.mode === 'montecarlo' && results.montecarlo && <MonteCarloView result={results.montecarlo} title={titleOf(results.montecarlo)} />}
        {form.mode === 'portfolio' && results.portfolio && (
          <PortfolioView result={results.portfolio} model={portfolioModels.find((m) => m.id === results.portfolio?.model)} />
        )}
      </section>
    </div>
  )
}
