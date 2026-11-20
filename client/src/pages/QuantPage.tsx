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
  type StrategyMeta,
  type StrategyParam,
  type WalkForwardResult,
} from '../api/quant'
import type { Market, WatchItem } from '../api/types'
import BacktestView from '../components/quant/BacktestView'
import MonteCarloView from '../components/quant/MonteCarloView'
import OptimizeView from '../components/quant/OptimizeView'
import WalkForwardView from '../components/quant/WalkForwardView'

type Mode = 'backtest' | 'optimize' | 'walkforward' | 'montecarlo'

const MODES: { id: Mode; label: string; hint: string }[] = [
  { id: 'backtest', label: '백테스트', hint: '설정 하나로 과거 성과 확인' },
  { id: 'optimize', label: '최적화', hint: '파라미터 조합을 격자로 비교' },
  { id: 'walkforward', label: '워크포워드', hint: '처음 보는 기간에도 통하는지 검증' },
  { id: 'montecarlo', label: '몬테카를로', hint: '매매 순서·구성의 운을 제거' },
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
  const [objectives, setObjectives] = useState<Record<string, string>>({})
  const [watchlist, setWatchlist] = useState<WatchItem[]>([])
  const [costs, setCosts] = useState<CostDefaults | null>(null)
  const [running, setRunning] = useState(false)
  const [error, setError] = useState('')
  const [results, setResults] = useState<{
    backtest?: BacktestResult
    optimize?: OptimizeResult
    walkforward?: WalkForwardResult
    montecarlo?: MonteCarloResult
  }>({})

  useEffect(() => {
    quantApi
      .strategies()
      .then(({ items, objectives }) => {
        setStrategies(items)
        setObjectives(objectives)
      })
      .catch((e) => setError(e.message))
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
      <form className="quant-form" onSubmit={submit}>
        <h2>퀀트 랩</h2>
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
            {strategies.map((s) => (
              <option key={s.id} value={s.id}>{s.name}</option>
            ))}
          </select>
        </label>
        {meta && <p className="muted small strategy-desc">{meta.description}</p>}

        {meta && !gridMode && (
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

        <button type="submit" disabled={running || !meta || !costs || !form.symbol.trim()}>
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
      </section>
    </div>
  )
}
