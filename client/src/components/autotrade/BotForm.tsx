import { useEffect, useState, type FormEvent } from 'react'
import { type Bot, type BotInput } from '../../api/autotrade'
import { marketApi } from '../../api/market'
import type { StrategyMeta } from '../../api/quant'
import type { Market, WatchItem } from '../../api/types'

interface Props {
  strategies: StrategyMeta[]
  initial?: Partial<BotInput> & { id?: string }
  bot?: Bot
  onSubmit: (b: BotInput) => Promise<void>
  onClose: () => void
}

export default function BotForm({ strategies, initial, bot, onSubmit, onClose }: Props) {
  const [market, setMarket] = useState<Market>(initial?.market ?? 'KR')
  const [symbol, setSymbol] = useState(initial?.symbol ?? '')
  const [strategy, setStrategy] = useState(initial?.strategy ?? strategies[0]?.id ?? '')
  const [params, setParams] = useState<Record<string, number>>(initial?.params ?? {})
  const [capital, setCapital] = useState(initial?.capital ?? (market === 'KR' ? 1_000_000 : 1_000))
  const [name, setName] = useState(initial?.name ?? '')
  const [watch, setWatch] = useState<WatchItem[]>([])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const holding = !!bot && (bot.state.position.qty > 0)

  useEffect(() => {
    marketApi.watchlist().then((r) => setWatch(r.items)).catch(() => undefined)
  }, [])
  const meta = strategies.find((s) => s.id === strategy)
  const value = (k: string, d: number) => params[k] ?? d

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError('')
    try {
      const full = Object.fromEntries((meta?.params ?? []).map((p) => [p.key, value(p.key, p.default)]))
      await onSubmit({ name: name || undefined, market, symbol: symbol.trim().toUpperCase(), strategy, params: full, capital })
      onClose()
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <form className="modal" onSubmit={submit}>
        <h3>{bot ? '봇 설정 변경' : '새 자동매매 봇'}</h3>
        <p className="muted small">새 봇은 <b>모의 모드·꺼진 상태</b>로 만들어집니다. 퀀트 랩에서 검증한 설정을 쓰는 걸 권장합니다.</p>
        <div className="modal-grid">
          <label>
            <span>시장</span>
            <select value={market} disabled={holding} onChange={(e) => setMarket(e.target.value as Market)}>
              <option value="KR">국내</option>
              <option value="US">미국</option>
            </select>
          </label>
          <label>
            <span>종목 코드</span>
            <input list="bot-watch" value={symbol} disabled={holding} onChange={(e) => setSymbol(e.target.value.toUpperCase())} placeholder={market === 'KR' ? '005930' : 'AAPL'} />
            <datalist id="bot-watch">
              {watch.filter((w) => w.market === market).map((w) => <option key={w.symbol} value={w.symbol}>{w.name}</option>)}
            </datalist>
          </label>
          <label>
            <span>전략</span>
            <select value={strategy} disabled={holding} onChange={(e) => { setStrategy(e.target.value); setParams({}) }}>
              {strategies.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}
            </select>
          </label>
          <label>
            <span>운용 자금 ({market === 'KR' ? '원' : 'USD'}) <span className="muted small">· 이 봇이 쓸 수 있는 최대 금액</span></span>
            <input type="number" min={1} value={capital} onChange={(e) => setCapital(Number(e.target.value))} />
          </label>
          {meta?.params.map((p) => (
            <label key={p.key}>
              <span>{p.label}</span>
              <input type="number" step={p.step} min={p.min} max={p.max} value={value(p.key, p.default)}
                     onChange={(e) => setParams({ ...params, [p.key]: Number(e.target.value) })} />
            </label>
          ))}
          <label>
            <span>이름 <span className="muted small">(선택)</span></span>
            <input value={name} onChange={(e) => setName(e.target.value)} placeholder={meta ? `${meta.name} · ${symbol}` : ''} />
          </label>
        </div>
        {meta && <p className="muted small">{meta.summary}</p>}
        {error && <p className="error">{error}</p>}
        <div className="modal-actions">
          <button type="button" onClick={onClose}>취소</button>
          <button type="submit" disabled={busy || !symbol.trim()}>{busy ? '저장 중…' : bot ? '저장' : '만들기'}</button>
        </div>
      </form>
    </div>
  )
}
