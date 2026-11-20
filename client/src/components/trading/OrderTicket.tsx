import { useEffect, useState } from 'react'
import { accountApi } from '../../api/account'
import { ApiError } from '../../api/client'
import { tradingApi, type OrderPreview, type OrderType, type Side, type TradingStatus } from '../../api/trading'
import type { Market } from '../../api/types'
import { priceFormatter } from '../chart/format'
import UnlockBar from './UnlockBar'

interface Props {
  market: Market
  symbol: string
  name?: string
  lastPrice: number | null
  pickedPrice: number | null // set when a price in the orderbook is clicked
}

/** Manual order: preview (server-side checks, nothing sent) -> send (needs the OTP trading unlock). */
export default function OrderTicket({ market, symbol, name, lastPrice, pickedPrice }: Props) {
  const fmt = priceFormatter(market)
  const [side, setSide] = useState<Side>('BUY')
  const [type, setType] = useState<OrderType>('LIMIT')
  const [price, setPrice] = useState<number>(lastPrice ?? 0)
  const [qty, setQty] = useState(1)
  const [highValue, setHighValue] = useState(false)
  const [preview, setPreview] = useState<OrderPreview | null>(null)
  const [status, setStatus] = useState<TradingStatus | null>(null)
  const [cash, setCash] = useState<number | null>(null)
  const [held, setHeld] = useState(0)
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null)

  const loadAccount = () =>
    accountApi
      .summary()
      .then((s) => {
        setCash(market === 'KR' ? s.buying_power.KRW : s.buying_power.USD)
        setHeld(s.holdings.items.find((i) => i.symbol === symbol)?.quantity ?? 0)
      })
      .catch(() => undefined)

  useEffect(() => {
    setPreview(null)
    setMsg(null)
    setPrice(lastPrice ?? 0)
    loadAccount()
    const refresh = () => loadAccount()
    window.addEventListener('mt:order-event', refresh)
    return () => window.removeEventListener('mt:order-event', refresh)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [market, symbol])

  useEffect(() => {
    if (pickedPrice) {
      setPrice(pickedPrice)
      setType('LIMIT')
      setPreview(null)
    }
  }, [pickedPrice])

  const changed = <T,>(set: (v: T) => void) => (v: T) => {
    set(v)
    setPreview(null)
    setMsg(null)
  }
  const unitPrice = type === 'LIMIT' ? price : lastPrice ?? price
  const pct = (p: number) => {
    const n = side === 'BUY' ? Math.floor(((cash ?? 0) * p) / (unitPrice * 1.005 || 1)) : Math.floor(held * p)
    changed(setQty)(Math.max(n, 0))
  }

  const send = async (live: boolean) => {
    setBusy(true)
    setMsg(null)
    try {
      const r = await tradingApi.order({
        market, symbol, side, order_type: type, quantity: qty, confirm_high_value: highValue,
        ...(type === 'LIMIT' ? { price } : {}), dry_run: !live,
      })
      if (live) {
        setPreview(null)
        setMsg({ ok: true, text: `주문 접수됨 (${r.result?.orderId ?? ''})` })
        loadAccount()
        window.dispatchEvent(new Event('mt:order-sent'))
      } else {
        setPreview(r)
      }
    } catch (e) {
      if (e instanceof ApiError && e.code === 'confirm_high_value_required') setHighValue(false)
      setMsg({ ok: false, text: (e as Error).message })
    } finally {
      setBusy(false)
    }
  }

  const est = qty * (unitPrice || 0)
  return (
    <div className="ticket">
      <UnlockBar onChange={setStatus} />
      <div className="seg">
        <button className={side === 'BUY' ? 'buy active' : ''} onClick={() => changed(setSide)('BUY')}>매수</button>
        <button className={side === 'SELL' ? 'sell active' : ''} onClick={() => changed(setSide)('SELL')}>매도</button>
      </div>
      <div className="seg small-seg">
        <button className={type === 'LIMIT' ? 'active' : ''} onClick={() => changed(setType)('LIMIT')}>지정가</button>
        <button className={type === 'MARKET' ? 'active' : ''} onClick={() => changed(setType)('MARKET')}>시장가</button>
      </div>
      <label>
        <span>가격 {type === 'LIMIT' ? <span className="muted small">· 호가를 누르면 입력돼요</span> : <span className="muted small">· 시장가 (현재 {lastPrice ? fmt(lastPrice) : '-'})</span>}</span>
        <input type="number" step="any" value={type === 'LIMIT' ? price : ''} disabled={type === 'MARKET'}
               onChange={(e) => changed(setPrice)(Number(e.target.value))} />
      </label>
      <label>
        <span>수량 <span className="muted small">· {side === 'BUY' ? `매수 가능 ${cash != null ? fmt(cash) : '-'}` : `보유 ${held.toLocaleString()}주`}</span></span>
        <input type="number" min={1} step={1} value={qty} onChange={(e) => changed(setQty)(Math.max(0, Math.floor(Number(e.target.value))))} />
      </label>
      <div className="seg pct">
        {[0.1, 0.25, 0.5, 1].map((p) => <button key={p} onClick={() => pct(p)}>{p * 100}%</button>)}
      </div>
      <div className="ticket-sum small">
        <span className="muted">예상 금액</span>
        <b>{fmt(est)} {market === 'KR' ? '원' : 'USD'}</b>
      </div>
      {preview && (
        <div className="ticket-preview small">
          <div><b>{name ?? symbol}</b> {side === 'BUY' ? '매수' : '매도'} {preview.payload.quantity}주 · {type === 'LIMIT' ? `${fmt(Number(preview.payload.price))} 지정가` : '시장가'}</div>
          {preview.warnings.map((w) => <div key={w} className="warn-text">⚠ {w}</div>)}
          <div className="muted">아직 전송되지 않았습니다.</div>
        </div>
      )}
      {msg?.text.includes('1억') && (
        <label className="check small"><input type="checkbox" checked={highValue} onChange={(e) => changed(setHighValue)(e.target.checked)} /> 1억 원 이상 고액 주문 확인</label>
      )}
      {!preview ? (
        <button className="ticket-go" disabled={busy || qty <= 0 || (type === 'LIMIT' && price <= 0)} onClick={() => send(false)}>
          {busy ? '확인 중…' : '주문 확인'}
        </button>
      ) : (
        <button className={`ticket-go ${side === 'BUY' ? 'buy' : 'sell'}`} disabled={busy || !status?.unlocked} onClick={() => send(true)}>
          {status?.unlocked ? `${side === 'BUY' ? '매수' : '매도'} 주문 전송` : '먼저 OTP로 거래 잠금을 해제하세요'}
        </button>
      )}
      {msg && <p className={msg.ok ? 'ok small' : 'error small'}>{msg.text}</p>}
    </div>
  )
}
