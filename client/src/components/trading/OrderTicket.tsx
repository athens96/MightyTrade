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
        <button className={side === 'BUY' ? 'buy active' : ''} title="주식을 새로 사들이는 주문입니다" onClick={() => changed(setSide)('BUY')}>매수</button>
        <button className={side === 'SELL' ? 'sell active' : ''} title="보유 중인 주식을 파는 주문입니다" onClick={() => changed(setSide)('SELL')}>매도</button>
      </div>
      <div className="seg small-seg">
        <button className={type === 'LIMIT' ? 'active' : ''} title="내가 직접 정한 가격 이하(매수) 또는 이상(매도)에서만 체결되도록 안전하게 가격을 지정합니다" onClick={() => changed(setType)('LIMIT')}>지정가</button>
        <button className={type === 'MARKET' ? 'active' : ''} title="가격을 정하지 않고 지금 시장에서 바로 체결 가능한 가격으로 즉시 거래합니다" onClick={() => changed(setType)('MARKET')}>시장가</button>
      </div>
      <label title={type === 'LIMIT' ? '원하는 1주 주문 가격을 입력하거나 위 호가창의 가격 줄을 클릭해 자동 입력합니다' : '시장가 주문은 현재 시장 가격으로 바로 체결되므로 가격을 따로 입력하지 않습니다'}>
        <span>가격 {type === 'LIMIT' ? <span className="muted small">· 호가를 누르면 입력돼요</span> : <span className="muted small">· 시장가 (현재 {lastPrice ? fmt(lastPrice) : '-'})</span>}</span>
        <input type="number" step="any" value={type === 'LIMIT' ? price : ''} disabled={type === 'MARKET'}
               title={type === 'LIMIT' ? '원하는 1주 주문 가격을 입력하거나 위 호가창의 가격 줄을 클릭해 자동 입력합니다' : '시장가 주문은 가격을 입력하지 않습니다'}
               onChange={(e) => changed(setPrice)(Number(e.target.value))} />
      </label>
      <label title="사고팔 주식 수량(주 단위)을 입력합니다">
        <span>수량 <span className="muted small">· {side === 'BUY' ? `매수 가능 ${cash != null ? fmt(cash) : '-'}` : `보유 ${held.toLocaleString()}주`}</span></span>
        <input type="number" min={1} step={1} value={qty} title="사고팔 주식 수량(주 단위)을 입력합니다" onChange={(e) => changed(setQty)(Math.max(0, Math.floor(Number(e.target.value))))} />
      </label>
      <div className="seg pct">
        {[0.1, 0.25, 0.5, 1].map((p) => (
          <button
            key={p}
            title={side === 'BUY' ? `매수 가능 현금의 ${p * 100}%만큼 살 수 있는 주식 수를 자동 계산해 채웁니다` : `현재 보유 수량의 ${p * 100}%를 자동 계산해 채웁니다`}
            onClick={() => pct(p)}
          >
            {p * 100}%
          </button>
        ))}
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
        <label className="check small" title="1억 원 이상 큰 금액의 주문 실수를 막기 위한 최종 확인 체크입니다"><input type="checkbox" checked={highValue} onChange={(e) => changed(setHighValue)(e.target.checked)} /> 1억 원 이상 고액 주문 확인</label>
      )}
      {!preview ? (
        <button
          className="ticket-go"
          disabled={busy || qty <= 0 || (type === 'LIMIT' && price <= 0)}
          title="아직 실제 주문을 보내지 않고, 잔고·호가 단위·상하한가·장 운영 여부를 먼저 안전하게 점검합니다"
          onClick={() => send(false)}
        >
          {busy ? '확인 중…' : '주문 확인'}
        </button>
      ) : (
        <button
          className={`ticket-go ${side === 'BUY' ? 'buy' : 'sell'}`}
          disabled={busy || !status?.unlocked}
          title={status?.unlocked ? '점검을 마친 주문을 토스증권으로 실제 전송합니다' : '실제 주문을 보내려면 위 잠금 바에서 OTP 6자리를 입력해 거래 잠금을 먼저 해제하세요'}
          onClick={() => send(true)}
        >
          {status?.unlocked ? `${side === 'BUY' ? '매수' : '매도'} 주문 전송` : '먼저 OTP로 거래 잠금을 해제하세요'}
        </button>
      )}
      {msg && <p className={msg.ok ? 'ok small' : 'error small'}>{msg.text}</p>}
    </div>
  )
}
