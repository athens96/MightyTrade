import { useEffect, useState, type FormEvent } from 'react'
import type { Bot, BotCategory, BotInput } from '../../api/autotrade'
import { marketApi } from '../../api/market'
import type { PortfolioAsset, PortfolioModelMeta, RebalanceMode, StrategyMeta } from '../../api/quant'
import type { Market, WatchItem } from '../../api/types'

interface Props {
  strategies: StrategyMeta[]
  portfolioModels: PortfolioModelMeta[]
  initial?: Partial<BotInput> & { id?: string; category?: BotCategory }
  bot?: Bot
  onSubmit: (b: BotInput) => Promise<void>
  onClose: () => void
}

const REBAL_OPTIONS: { id: RebalanceMode; label: string }[] = [
  { id: 'monthly', label: '매월 초' },
  { id: 'quarterly', label: '분기 초' },
  { id: 'yearly', label: '매년 초' },
  { id: 'drift', label: '비중 편차 이탈 시' },
]

export default function BotForm({ strategies, portfolioModels, initial, bot, onSubmit, onClose }: Props) {
  const initialCat: BotCategory =
    bot?.category ??
    (initial?.kind === 'rebalance'
      ? 'rebalance'
      : initial?.category ?? strategies.find((s) => s.id === initial?.strategy)?.category ?? 'swing')

  const [category, setCategory] = useState<BotCategory>(initialCat)
  const [market, setMarket] = useState<Market>(initial?.market ?? (initialCat === 'rebalance' ? 'US' : 'KR'))
  const [symbol, setSymbol] = useState(initial?.symbol ?? '')
  const [strategy, setStrategy] = useState(
    initial?.strategy ?? strategies.find((s) => s.category === (initialCat === 'daytrade' ? 'daytrade' : 'swing'))?.id ?? strategies[0]?.id ?? '',
  )
  const [modelId, setModelId] = useState(initial?.model ?? portfolioModels[0]?.id ?? 'all_weather')
  const [rebalance, setRebalance] = useState<RebalanceMode>(
    initial?.rebalance ?? portfolioModels.find((m) => m.id === (initial?.model ?? 'all_weather'))?.default_rebalance ?? 'monthly',
  )
  const [driftPct, setDriftPct] = useState(initial?.drift_pct ?? 5)
  const [assets, setAssets] = useState<PortfolioAsset[]>(
    initial?.assets ?? portfolioModels.find((m) => m.id === (initial?.model ?? 'all_weather'))?.presets[initial?.market ?? 'US'] ?? [],
  )
  const [params, setParams] = useState<Record<string, number>>(initial?.params ?? {})
  const [capital, setCapital] = useState(initial?.capital ?? (market === 'KR' ? 1_000_000 : 1_000))
  const [name, setName] = useState(initial?.name ?? '')
  const [watch, setWatch] = useState<WatchItem[]>([])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  const holding = !!bot && (
    bot.kind === 'rebalance'
      ? Object.values(bot.state.positions ?? {}).some((p) => p.qty > 0)
      : bot.state.position.qty > 0
  )

  useEffect(() => {
    marketApi.watchlist().then((r) => setWatch(r.items)).catch(() => undefined)
  }, [])

  const isRebal = category === 'rebalance'
  const catStrategies = strategies.filter((s) => s.category === (category === 'daytrade' ? 'daytrade' : 'swing'))
  const meta = catStrategies.find((s) => s.id === strategy) ?? catStrategies[0]
  const pModel = portfolioModels.find((m) => m.id === modelId) ?? portfolioModels[0]
  const value = (k: string, d: number) => params[k] ?? d

  const switchCategory = (next: BotCategory) => {
    setCategory(next)
    setParams({})
    if (next === 'rebalance') {
      const m = portfolioModels.find((x) => x.id === modelId) ?? portfolioModels[0]
      if (m) {
        setModelId(m.id)
        setRebalance(m.default_rebalance)
        setAssets(m.presets[market] ?? [])
      }
    } else {
      const first = strategies.find((s) => s.category === next)
      if (first) setStrategy(first.id)
    }
  }

  const switchMarket = (nextMarket: Market) => {
    setMarket(nextMarket)
    if (!bot) setCapital(nextMarket === 'KR' ? 1_000_000 : 1_000)
    if (isRebal && pModel && !holding) setAssets(pModel.presets[nextMarket] ?? [])
  }

  const switchModel = (nextId: string) => {
    setModelId(nextId)
    setParams({})
    const m = portfolioModels.find((x) => x.id === nextId)
    if (m) {
      setRebalance(m.default_rebalance)
      setAssets(m.presets[market] ?? [])
    }
  }

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError('')
    try {
      if (isRebal) {
        if (!pModel) return
        const full = Object.fromEntries(pModel.params.map((p) => [p.key, value(p.key, p.default)]))
        await onSubmit({
          kind: 'rebalance',
          name: name || undefined,
          market,
          model: pModel.id,
          assets,
          rebalance,
          drift_pct: driftPct,
          params: full,
          capital,
        })
      } else {
        if (!meta) return
        const full = Object.fromEntries(meta.params.map((p) => [p.key, value(p.key, p.default)]))
        await onSubmit({
          kind: 'single',
          name: name || undefined,
          market,
          symbol: symbol.trim().toUpperCase(),
          strategy: meta.id,
          params: full,
          capital,
        })
      }
      onClose()
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const canSubmit = isRebal ? assets.length >= 2 && assets.every((a) => a.symbol.trim()) : !!symbol.trim()

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <form className="modal" onSubmit={submit}>
        <h3>{bot ? '봇 설정 변경' : '새 자동매매 봇'}</h3>
        <p className="muted small">새 봇은 <b>모의 모드·꺼진 상태</b>로 만들어집니다. 퀀트 랩에서 검증한 이론·설정을 쓰는 걸 권장합니다.</p>

        {!bot && (
          <div className="tabs" style={{ marginBottom: 12 }}>
            {([
              ['swing', '스윙봇 (일봉)'],
              ['daytrade', '단타봇 (장중·분봉)'],
              ['rebalance', '리밸런싱 봇 (포트폴리오)'],
            ] as [BotCategory, string][]).map(([id, label]) => (
              <button type="button" key={id} className={category === id ? 'active' : ''} onClick={() => switchCategory(id)}>
                {label}
              </button>
            ))}
          </div>
        )}

        <div className="modal-grid">
          <label>
            <span>시장</span>
            <select value={market} disabled={holding} onChange={(e) => switchMarket(e.target.value as Market)}>
              <option value="KR">국내</option>
              <option value="US">미국</option>
            </select>
          </label>

          {!isRebal && (
            <>
              <label>
                <span>종목 코드</span>
                <input list="bot-watch" value={symbol} disabled={holding} onChange={(e) => setSymbol(e.target.value.toUpperCase())} placeholder={market === 'KR' ? '005930' : 'AAPL'} />
                <datalist id="bot-watch">
                  {watch.filter((w) => w.market === market).map((w) => <option key={w.symbol} value={w.symbol}>{w.name}</option>)}
                </datalist>
              </label>
              <label>
                <span>금융 이론 전략</span>
                <select value={meta?.id ?? strategy} disabled={holding} onChange={(e) => { setStrategy(e.target.value); setParams({}) }}>
                  {catStrategies.map((s) => (
                    <option key={s.id} value={s.id}>{s.name}{s.timeframe !== '1d' ? ` (${s.timeframe})` : ''}</option>
                  ))}
                </select>
              </label>
            </>
          )}

          {isRebal && pModel && (
            <>
              <label>
                <span>자산배분 이론</span>
                <select value={pModel.id} disabled={holding} onChange={(e) => switchModel(e.target.value)}>
                  {portfolioModels.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
                </select>
              </label>
              <label>
                <span>리밸런싱 주기</span>
                <select value={rebalance} onChange={(e) => setRebalance(e.target.value as RebalanceMode)}>
                  {REBAL_OPTIONS.map((o) => <option key={o.id} value={o.id}>{o.label}</option>)}
                </select>
              </label>
              {rebalance === 'drift' && (
                <label>
                  <span>허용 비중 편차 (%)</span>
                  <input type="number" min={0.5} max={50} step={0.5} value={driftPct} onChange={(e) => setDriftPct(Number(e.target.value))} />
                </label>
              )}
            </>
          )}

          <label>
            <span>운용 자금 ({market === 'KR' ? '원' : 'USD'}) <span className="muted small">· 이 봇의 최대 자금</span></span>
            <input type="number" min={1} value={capital} onChange={(e) => setCapital(Number(e.target.value))} />
          </label>

          {!isRebal && meta?.params.map((p) => (
            <label key={p.key}>
              <span>{p.label}</span>
              <input type="number" step={p.step} min={p.min} max={p.max} value={value(p.key, p.default)}
                     onChange={(e) => setParams({ ...params, [p.key]: Number(e.target.value) })} />
            </label>
          ))}

          {isRebal && pModel?.params.map((p) => (
            <label key={p.key}>
              <span>{p.label}</span>
              <input type="number" step={p.step} min={p.min} max={p.max} value={value(p.key, p.default)}
                     onChange={(e) => setParams({ ...params, [p.key]: Number(e.target.value) })} />
            </label>
          ))}

          <label>
            <span>이름 <span className="muted small">(선택)</span></span>
            <input value={name} onChange={(e) => setName(e.target.value)}
                   placeholder={isRebal ? (pModel ? `${pModel.name} (${market})` : '') : (meta ? `${meta.name} · ${symbol}` : '')} />
          </label>
        </div>

        {isRebal && pModel && (
          <div style={{ marginTop: 10 }}>
            <div className="small muted" style={{ marginBottom: 6 }}>
              <b>편입 자산 ({assets.length}종목)</b> {holding && '· 보유 중에는 종목 변경 불가'}
            </div>
            <div className="asset-editor">
              {assets.map((a, idx) => (
                <div key={idx} className="asset-row">
                  <input value={a.symbol} disabled={holding} placeholder={market === 'KR' ? '069500' : 'SPY'}
                         onChange={(e) => setAssets(assets.map((x, i) => (i === idx ? { ...x, symbol: e.target.value.toUpperCase() } : x)))} />
                  {pModel.uses_roles && (
                    <select value={a.role} disabled={holding}
                            onChange={(e) => setAssets(assets.map((x, i) => (i === idx ? { ...x, role: e.target.value as 'offensive' | 'defensive' } : x)))}>
                      <option value="offensive">공격</option>
                      <option value="defensive">방어</option>
                    </select>
                  )}
                  {pModel.uses_weights && (
                    <input type="number" min={0} max={100} step={0.5} value={a.weight} title="목표 비중 (%)"
                           onChange={(e) => setAssets(assets.map((x, i) => (i === idx ? { ...x, weight: Number(e.target.value) } : x)))} />
                  )}
                  {!holding && (
                    <button type="button" className="link danger-link" disabled={assets.length <= 2}
                            onClick={() => setAssets(assets.filter((_, i) => i !== idx))}>×</button>
                  )}
                </div>
              ))}
              {!holding && (
                <div className="tabs">
                  <button type="button" disabled={assets.length >= 12}
                          onClick={() => setAssets([...assets, { symbol: '', role: 'offensive', weight: 10 }])}>+ 종목 추가</button>
                  <button type="button" onClick={() => setAssets(pModel.presets[market] ?? [])}>기본 프리셋</button>
                </div>
              )}
            </div>
          </div>
        )}

        {!isRebal && meta && (
          <p className="muted small" style={{ marginTop: 10 }}>
            <b>{meta.theory}</b><br />{meta.summary}
          </p>
        )}
        {isRebal && pModel && (
          <p className="muted small" style={{ marginTop: 10 }}>
            <b>{pModel.theory}</b><br />{pModel.summary}
          </p>
        )}
        {error && <p className="error">{error}</p>}
        <div className="modal-actions">
          <button type="button" onClick={onClose}>취소</button>
          <button type="submit" disabled={busy || !canSubmit}>{busy ? '저장 중…' : bot ? '저장' : '만들기'}</button>
        </div>
      </form>
    </div>
  )
}
