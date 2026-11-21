import { useCallback, useEffect, useState } from 'react'
import { ORDER_STATUS, tradingApi, type Order } from '../../api/trading'
import { priceFormatter } from '../chart/format'
import UnlockBar from './UnlockBar'

const time = (iso: string) => new Date(iso).toLocaleString('ko-KR', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hour12: false })

/** Open orders (modify/cancel) and recent filled/cancelled orders. */
export default function OrdersSection({ nameOf }: { nameOf: (symbol: string) => string }) {
  const [status, setStatus] = useState<'OPEN' | 'CLOSED'>('OPEN')
  const [items, setItems] = useState<Order[]>([])
  const [error, setError] = useState('')

  const load = useCallback(() => {
    tradingApi.orders(status).then((r) => {
      setItems(r.items)
      setError('')
    }).catch((e) => setError(e.message))
  }, [status])

  useEffect(() => {
    load()
    const on = () => load()
    window.addEventListener('mt:order-event', on)
    window.addEventListener('mt:order-sent', on)
    return () => {
      window.removeEventListener('mt:order-event', on)
      window.removeEventListener('mt:order-sent', on)
    }
  }, [load])

  const act = async (fn: () => Promise<unknown>) => {
    try {
      await fn()
      load()
    } catch (e) {
      alert((e as Error).message)
    }
  }
  const modify = (o: Order) => {
    const v = prompt(`새 가격 (현재 ${o.price ?? '-'})`, String(o.price ?? ''))
    if (v) act(() => tradingApi.modify(o.orderId, { price: Number(v), order_type: 'LIMIT' }))
  }

  return (
    <>
      <div className="account-head">
        <h4>주문 내역</h4>
        <div className="tabs">
          {(['OPEN', 'CLOSED'] as const).map((s) => (
            <button
              key={s}
              className={status === s ? 'active' : ''}
              title={s === 'OPEN' ? '아직 체결되지 않고 호가창에 대기 중인 주문을 확인하고 가격 정정이나 취소를 합니다' : '최근 30일 동안 체결되었거나 취소된 일반 주문 내역을 확인합니다'}
              onClick={() => setStatus(s)}
            >
              {s === 'OPEN' ? '미체결' : '체결·취소 (30일)'}
            </button>
          ))}
        </div>
        <div className="spacer" />
        <UnlockBar />
      </div>
      {error && <p className="error small">{error}</p>}
      <div className="trades-wrap">
        <table className="trades left-first cards">
          <thead>
            <tr><th>종목</th><th>주문 시각</th><th>구분</th><th>가격</th><th>수량</th><th>체결</th><th>상태</th><th /></tr>
          </thead>
          <tbody>
            {items.map((o) => {
              const fmt = priceFormatter(o.currency === 'KRW' ? 'KR' : 'US')
              const open = status === 'OPEN'
              return (
                <tr key={o.orderId}>
                  <td className="card-title"><b>{nameOf(o.symbol)}</b> <span className="muted small">{o.symbol}</span></td>
                  <td data-label="주문 시각" className="muted">{time(o.orderedAt)}</td>
                  <td data-label="구분" className={o.side === 'BUY' ? 'up' : 'down'}>{o.side === 'BUY' ? '매수' : '매도'} {o.orderType === 'MARKET' ? '시장가' : '지정가'}</td>
                  <td data-label="가격">{o.price != null ? fmt(o.price) : '-'}</td>
                  <td data-label="수량">{o.quantity.toLocaleString()}</td>
                  <td data-label="체결">{o.execution.filledQuantity ? `${o.execution.filledQuantity.toLocaleString()} @ ${fmt(o.execution.averageFilledPrice ?? 0)}` : '-'}</td>
                  <td data-label="상태">{ORDER_STATUS[o.status] ?? o.status}</td>
                  <td className="card-actions">
                    {open && o.orderType === 'LIMIT' && (
                      <button className="link" title="아직 체결되지 않은 지정가 주문의 희망 가격을 변경합니다 (거래 잠금 해제 필요)" onClick={() => modify(o)}>정정</button>
                    )}
                    {open && (
                      <button className="link danger-link" title="아직 체결되지 않고 대기 중인 이 주문을 취소합니다" onClick={() => confirm('주문을 취소할까요?') && act(() => tradingApi.cancel(o.orderId))}>취소</button>
                    )}
                  </td>
                </tr>
              )
            })}
            {!items.length && <tr><td colSpan={8} className="muted">{status === 'OPEN' ? '미체결 주문이 없습니다.' : '최근 30일 주문이 없습니다.'}</td></tr>}
          </tbody>
        </table>
      </div>
    </>
  )
}
