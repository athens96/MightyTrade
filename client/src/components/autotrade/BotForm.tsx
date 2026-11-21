import { useEffect, useState, type FormEvent } from 'react'
import type { Bot, BotCategory, BotInput } from '../../api/autotrade'
import { marketApi } from '../../api/market'
import type { PortfolioAsset, PortfolioModelMeta, PortfolioRole, RebalanceMode, StrategyMeta } from '../../api/quant'
import type { Market, WatchItem } from '../../api/types'
import { getParamHint } from '../../lib/paramHints'
import { StrategyGuideBox } from '../GuideBox'

const ROLE_LABELS: Record<PortfolioRole, string> = {
  offensive: '공격',
  defensive: '방어',
  canary: '감시병',
  core: '고정',
}

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
  const [stopLossPct, setStopLossPct] = useState(initial?.risk?.stop_loss_pct ?? bot?.risk?.stop_loss_pct ?? 0)
  const [takeProfitPct, setTakeProfitPct] = useState(initial?.risk?.take_profit_pct ?? bot?.risk?.take_profit_pct ?? 0)
  const [trailingStopPct, setTrailingStopPct] = useState(initial?.risk?.trailing_stop_pct ?? bot?.risk?.trailing_stop_pct ?? 0)
  const [maxDdPct, setMaxDdPct] = useState(initial?.guard?.max_drawdown_pct ?? bot?.guard?.max_drawdown_pct ?? 0)
  const [maxLossStreak, setMaxLossStreak] = useState(initial?.guard?.max_consecutive_losses ?? bot?.guard?.max_consecutive_losses ?? 0)
  const [cooldownDays, setCooldownDays] = useState(initial?.guard?.cooldown_days ?? bot?.guard?.cooldown_days ?? 0)
  const [dailyLossPct, setDailyLossPct] = useState(initial?.guard?.daily_loss_limit_pct ?? bot?.guard?.daily_loss_limit_pct ?? 0)
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
    const guard = {
      max_drawdown_pct: maxDdPct || 0,
      max_consecutive_losses: maxLossStreak || 0,
      cooldown_days: cooldownDays || 0,
      daily_loss_limit_pct: dailyLossPct || 0,
    }
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
          guard,
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
          risk: {
            stop_loss_pct: stopLossPct || 0,
            take_profit_pct: takeProfitPct || 0,
            trailing_stop_pct: trailingStopPct || 0,
          },
          guard,
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
              ['swing', '스윙봇 (일봉)', '하루 한 번 일봉 종가를 보고 며칠~몇 주 추세를 타는 스윙 매매 봇입니다'],
              ['daytrade', '단타봇 (장중·분봉)', '장중 분봉이나 시초가 돌파를 노리고 장 마감 전 당일 모두 청산하는 단타 봇입니다'],
              ['rebalance', '리밸런싱 봇 (포트폴리오)', '여러 주식·채권·금 ETF를 담아 매월·분기마다 목표 비중을 자동으로 맞춰주는 포트폴리오 봇입니다'],
            ] as [BotCategory, string, string][]).map(([id, label, tip]) => (
              <button type="button" key={id} className={category === id ? 'active' : ''} title={tip} onClick={() => switchCategory(id)}>
                {label}
              </button>
            ))}
          </div>
        )}

        <div className="modal-grid">
          <label title="봇이 거래할 시장(국내 KRX 또는 미국 나스닥·뉴욕)을 선택합니다 (보유 수량이 있을 때는 변경 불가)">
            <span>시장</span>
            <select
              value={market}
              disabled={holding}
              title="봇이 거래할 시장(국내 KRX 또는 미국 나스닥·뉴욕)을 선택합니다"
              onChange={(e) => switchMarket(e.target.value as Market)}
            >
              <option value="KR">국내</option>
              <option value="US">미국</option>
            </select>
          </label>

          {!isRebal && (
            <>
              <label title="봇이 감시하고 매매할 종목 코드(예: 005930, AAPL)를 입력하거나 관심종목에서 고릅니다">
                <span>종목 코드</span>
                <input
                  list="bot-watch"
                  value={symbol}
                  disabled={holding}
                  title="봇이 감시하고 매매할 종목 코드(예: 005930, AAPL)를 입력하거나 관심종목에서 고릅니다"
                  onChange={(e) => setSymbol(e.target.value.toUpperCase())}
                  placeholder={market === 'KR' ? '005930' : 'AAPL'}
                />
                <datalist id="bot-watch">
                  {watch.filter((w) => w.market === market).map((w) => <option key={w.symbol} value={w.symbol}>{w.name}</option>)}
                </datalist>
              </label>
              <label title="봇이 어떤 매매 공식에 따라 자동으로 사고팔지 금융 전략을 선택합니다">
                <span>금융 이론 전략</span>
                <select
                  value={meta?.id ?? strategy}
                  disabled={holding}
                  title="봇이 어떤 매매 공식에 따라 자동으로 사고팔지 금융 전략을 선택합니다"
                  onChange={(e) => { setStrategy(e.target.value); setParams({}) }}
                >
                  {catStrategies.map((s) => (
                    <option key={s.id} value={s.id}>{s.name}{s.timeframe !== '1d' ? ` (${s.timeframe})` : ''}</option>
                  ))}
                </select>
              </label>
            </>
          )}

          {isRebal && pModel && (
            <>
              <label title="여러 자산의 목표 비중을 계산할 자산배분 모델을 선택합니다">
                <span>자산배분 이론</span>
                <select value={pModel.id} disabled={holding} title="여러 자산의 목표 비중을 계산할 자산배분 모델을 선택합니다" onChange={(e) => switchModel(e.target.value)}>
                  {portfolioModels.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
                </select>
              </label>
              <label title="얼마나 자주 포트폴리오 비중을 점검하고 다시 맞출지 주기를 선택합니다">
                <span>리밸런싱 주기</span>
                <select value={rebalance} title="얼마나 자주 포트폴리오 비중을 점검하고 다시 맞출지 주기를 선택합니다" onChange={(e) => setRebalance(e.target.value as RebalanceMode)}>
                  {REBAL_OPTIONS.map((o) => <option key={o.id} value={o.id}>{o.label}</option>)}
                </select>
              </label>
              {rebalance === 'drift' && (
                <label title="목표 비중에서 몇 %p 이상 벗어났을 때만 사고팔아 비중을 다시 맞출지 정합니다">
                  <span>허용 비중 편차 (%)</span>
                  <input
                    type="number"
                    min={0.5}
                    max={50}
                    step={0.5}
                    value={driftPct}
                    title="목표 비중에서 몇 %p 이상 벗어났을 때만 사고팔아 비중을 다시 맞출지 정합니다"
                    onChange={(e) => setDriftPct(Number(e.target.value))}
                  />
                </label>
              )}
            </>
          )}

          <label title="이 봇 하나에 맡길 최대 운용 금액입니다 (계좌 전체 현금이 아니라 이 한도 안에서만 주문 수량을 계산합니다)">
            <span>운용 자금 ({market === 'KR' ? '원' : 'USD'}) <span className="muted small">· 이 봇의 최대 자금</span></span>
            <input
              type="number"
              min={1}
              value={capital}
              title="이 봇 하나에 맡길 최대 운용 금액입니다"
              onChange={(e) => setCapital(Number(e.target.value))}
            />
          </label>

          {!isRebal && meta?.params.map((p) => {
            const tip = getParamHint(meta.id, p.key, p.label, p.min, p.max)
            return (
              <label key={p.key} title={tip}>
                <span>{p.label}</span>
                <input type="number" step={p.step} min={p.min} max={p.max} value={value(p.key, p.default)} title={tip}
                       onChange={(e) => setParams({ ...params, [p.key]: Number(e.target.value) })} />
              </label>
            )
          })}

          {isRebal && pModel?.params.map((p) => {
            const tip = getParamHint(pModel.id, p.key, p.label, p.min, p.max)
            return (
              <label key={p.key} title={tip}>
                <span>{p.label}</span>
                <input type="number" step={p.step} min={p.min} max={p.max} value={value(p.key, p.default)} title={tip}
                       onChange={(e) => setParams({ ...params, [p.key]: Number(e.target.value) })} />
              </label>
            )
          })}

          <label title="봇 목록에서 구분하기 쉬운 별명을 입력합니다 (비워두면 전략과 종목 이름으로 자동 지정됩니다)">
            <span>이름 <span className="muted small">(선택)</span></span>
            <input value={name} title="봇 목록에서 구분하기 쉬운 별명을 입력합니다 (비워두면 자동 생성)" onChange={(e) => setName(e.target.value)}
                   placeholder={isRebal ? (pModel ? `${pModel.name} (${market})` : '') : (meta ? `${meta.name} · ${symbol}` : '')} />
          </label>
        </div>

        {!isRebal && (
          <fieldset className="costs" style={{ marginTop: 10 }}>
            <legend>🛡️ 공통 보호막 · 토스 OCO 자동 연동 (0이면 끔)</legend>
            <div className="modal-grid">
              <label title="매수가 대비 이 비율(%)만큼 떨어지면 즉시 손절합니다. 실거래 시 토스증권 조건주문으로 자동 등록됩니다">
                <span>공통 손절 (%)</span>
                <input type="number" min={0} max={80} step={0.5} value={stopLossPct}
                       title="매수가 대비 이 비율(%)만큼 하락하면 즉시 손절합니다 (0 = 끔)"
                       onChange={(e) => setStopLossPct(Math.max(0, Number(e.target.value)))} />
              </label>
              <label title="매수가 대비 이 비율(%)만큼 오르면 목표 수익을 확정합니다. 손절과 함께 켜면 토스 OCO 양방향 주문으로 등록됩니다">
                <span>공통 목표 익절 (%)</span>
                <input type="number" min={0} max={80} step={0.5} value={takeProfitPct}
                       title="매수가 대비 이 비율(%)만큼 상승하면 목표 익절합니다 (0 = 끔)"
                       onChange={(e) => setTakeProfitPct(Math.max(0, Number(e.target.value)))} />
              </label>
              <label title="주가가 오른 뒤 보유 중 최고가 대비 이 비율(%)만큼 밀리면 익절선을 따라 올리며 수익을 잠급니다">
                <span>트레일링 스톱 (%)</span>
                <input type="number" min={0} max={80} step={0.5} value={trailingStopPct}
                       title="보유 중 최고가 대비 이 비율(%)만큼 하락하면 수익 보존 매도합니다 (0 = 끔)"
                       onChange={(e) => setTrailingStopPct(Math.max(0, Number(e.target.value)))} />
              </label>
            </div>
          </fieldset>
        )}

        <fieldset className="costs" style={{ marginTop: 10 }}>
          <legend>🛑 서킷브레이커 · 연패 자동 휴식 (0이면 끔)</legend>
          <div className="modal-grid">
            <label title="봇 평가자산이 원금(또는 최고점) 대비 이 비율(%) 이상 줄어들면 즉시 봇을 끄고 보호 잠금합니다">
              <span>최대 허용 낙폭 (%)</span>
              <input type="number" min={0} max={80} step={1} value={maxDdPct}
                     title="봇 자산이 최고점 대비 이 비율(%) 이상 줄면 자동으로 봇을 일시정지합니다 (예: 10)"
                     onChange={(e) => setMaxDdPct(Math.max(0, Number(e.target.value)))} />
            </label>
            <label title="연속으로 손절이 이 횟수만큼 이어지면 시장과 전략이 안 맞는 시기로 보고 자동으로 봇을 멈춥니다">
              <span>연속 손절 한도 (회)</span>
              <input type="number" min={0} max={20} step={1} value={maxLossStreak}
                     title="연속 손절이 이 횟수에 도달하면 봇을 자동 중지합니다 (예: 3)"
                     onChange={(e) => setMaxLossStreak(Math.max(0, Math.floor(Number(e.target.value))))} />
            </label>
            <label title="손실로 청산한 직후 같은 종목에 곧바로 재진입해 연타를 맞지 않도록 며칠 동안 쉴지 정합니다">
              <span>손절 후 쿨다운 (일)</span>
              <input type="number" min={0} max={60} step={1} value={cooldownDays}
                     title="손절 직후 며칠 동안 신규 매수를 쉬어갈지 정합니다 (예: 3)"
                     onChange={(e) => setCooldownDays(Math.max(0, Math.floor(Number(e.target.value))))} />
            </label>
            <label title="하루 동안 전날 자산 대비 이 비율(%) 이상 손실이 나면 그날 남은 시간에는 신규 매수를 보류합니다">
              <span>일일 손실 한도 (%)</span>
              <input type="number" min={0} max={50} step={0.5} value={dailyLossPct}
                     title="전날 대비 당일 손실이 이 비율(%)을 넘으면 오늘 신규 매수를 중단합니다 (예: 5)"
                     onChange={(e) => setDailyLossPct(Math.max(0, Number(e.target.value)))} />
            </label>
          </div>
        </fieldset>

        {isRebal && pModel && (
          <div style={{ marginTop: 10 }}>
            <div className="small muted" style={{ marginBottom: 6 }}>
              <b>편입 자산 ({assets.length}종목)</b> {holding && '· 보유 중에는 종목 변경 불가'}
            </div>
            <div className="asset-editor">
              {assets.map((a, idx) => (
                <div key={idx} className="asset-row">
                  <input value={a.symbol} disabled={holding} placeholder={market === 'KR' ? '069500' : 'SPY'}
                         title="포트폴리오에 담을 주식·ETF 종목 코드를 입력합니다 (예: 069500, SPY)"
                         onChange={(e) => setAssets(assets.map((x, i) => (i === idx ? { ...x, symbol: e.target.value.toUpperCase() } : x)))} />
                  {pModel.uses_roles && (
                    <select value={a.role} disabled={holding}
                            title="이 종목의 역할을 지정합니다 (공격: 수익 추구 / 방어: 하락장 대피처 / 감시병: 시장 위험 감지기 / 고정: 상시 보유)"
                            onChange={(e) => setAssets(assets.map((x, i) => (i === idx ? { ...x, role: e.target.value as PortfolioRole } : x)))}>
                      {(pModel.allowed_roles ?? ['offensive', 'defensive']).map((role) => (
                        <option key={role} value={role}>{ROLE_LABELS[role]}</option>
                      ))}
                    </select>
                  )}
                  {pModel.uses_weights && (
                    <input type="number" min={0} max={100} step={0.5} value={a.weight} title="전체 운용 자금 중 이 종목을 몇 % 담을지 목표 비중을 입력합니다"
                           onChange={(e) => setAssets(assets.map((x, i) => (i === idx ? { ...x, weight: Number(e.target.value) } : x)))} />
                  )}
                  {!holding && (
                    <button type="button" className="link danger-link" disabled={assets.length <= 2}
                            title="이 종목을 포트폴리오에서 뺍니다 (최소 2종목 필요)"
                            onClick={() => setAssets(assets.filter((_, i) => i !== idx))}>×</button>
                  )}
                </div>
              ))}
              {!holding && (
                <div className="tabs">
                  <button type="button" disabled={assets.length >= 16}
                          title="포트폴리오에 새 종목 입력 칸을 추가합니다 (최대 16종목)"
                          onClick={() => setAssets([...assets, { symbol: '', role: (pModel.allowed_roles ?? ['offensive'])[0], weight: 10 }])}>+ 종목 추가</button>
                  <button type="button" title="선택한 자산배분 이론의 표준 추천 ETF 구성으로 채웁니다" onClick={() => setAssets(pModel.presets[market] ?? [])}>기본 프리셋</button>
                </div>
              )}
            </div>
          </div>
        )}

        {!isRebal && meta && (
          <>
            <p className="muted small" style={{ marginTop: 10 }}>
              <b>{meta.theory}</b><br />{meta.summary}
            </p>
            <StrategyGuideBox theory={meta.theory} guide={meta.guide} />
          </>
        )}
        {isRebal && pModel && (
          <>
            <p className="muted small" style={{ marginTop: 10 }}>
              <b>{pModel.theory}</b><br />{pModel.summary}
            </p>
            <StrategyGuideBox theory={pModel.theory} guide={pModel.guide} />
          </>
        )}
        {error && <p className="error">{error}</p>}
        <div className="modal-actions">
          <button type="button" title="변경한 내용을 저장하지 않고 창을 닫습니다" onClick={onClose}>취소</button>
          <button
            type="submit"
            disabled={busy || !canSubmit}
            title={bot ? '수정한 봇 설정을 저장합니다' : '모의 모드·꺼진 상태로 안전하게 새 자동매매 봇을 만듭니다'}
          >
            {busy ? '저장 중…' : bot ? '저장' : '만들기'}
          </button>
        </div>
      </form>
    </div>
  )
}
