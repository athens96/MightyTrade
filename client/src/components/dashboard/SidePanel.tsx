import { useEffect, useState } from 'react'
import { marketApi, type FlowRecord, type OrderBook, type Tick } from '../../api/market'
import type { Market } from '../../api/types'
import type { StreamState } from '../../hooks/useRealtime'
import { compact, priceFormatter } from '../chart/format'

const STATE_LABEL: Record<StreamState, string> = {
  connecting: '연결 중', connected: '실시간', reconnecting: '재연결 중', unavailable: '실시간 미제공',
}

function Book({ book, prevClose, market }: { book: OrderBook; prevClose: number | null; market: Market }) {
  const fmt = priceFormatter(market)
  const asks = [...book.asks.slice(0, 10)].reverse() // highest ask on top, best ask just above the spread
  const bids = book.bids.slice(0, 10)
  const max = Math.max(1, ...asks.map((l) => l.volume), ...bids.map((l) => l.volume))
  const rel = (p: number) => (prevClose ? ((p / prevClose - 1) * 100) : null)
  const row = (l: { price: number; volume: number }, side: 'ask' | 'bid', i: number) => {
    const r = rel(l.price)
    return (
      <div key={`${side}${i}`} className={`ob-row ${side}`}>
        <span className="ob-vol">
          {side === 'ask' && <i style={{ width: `${(l.volume / max) * 100}%` }} />}
          {side === 'ask' ? l.volume.toLocaleString() : ''}
        </span>
        <span className={`ob-price ${r === null ? '' : r > 0 ? 'up' : r < 0 ? 'down' : ''}`}>
          {fmt(l.price)}
          {r !== null && <small>{r > 0 ? '+' : ''}{r.toFixed(2)}%</small>}
        </span>
        <span className="ob-vol right">
          {side === 'bid' && <i style={{ width: `${(l.volume / max) * 100}%` }} />}
          {side === 'bid' ? l.volume.toLocaleString() : ''}
        </span>
      </div>
    )
  }
  const askTotal = book.asks.reduce((a, l) => a + l.volume, 0)
  const bidTotal = book.bids.reduce((a, l) => a + l.volume, 0)
  return (
    <div className="orderbook">
      {asks.map((l, i) => row(l, 'ask', i))}
      <div className="ob-spread" />
      {bids.map((l, i) => row(l, 'bid', i))}
      <div className="ob-total small">
        <span className="down">매도 잔량 {compact(askTotal)}</span>
        <span className="up">매수 잔량 {compact(bidTotal)}</span>
      </div>
    </div>
  )
}

function Ticks({ ticks, market }: { ticks: Tick[]; market: Market }) {
  const fmt = priceFormatter(market)
  return (
    <table className="ticks">
      <thead>
        <tr><th>시각</th><th>체결가</th><th>수량</th></tr>
      </thead>
      <tbody>
        {ticks.slice(0, 30).map((t, i) => {
          const prev = ticks[i + 1]?.price
          const c = prev === undefined || t.price === prev ? '' : t.price > prev ? 'up' : 'down'
          return (
            <tr key={`${t.timestamp}-${i}`}>
              <td className="muted">{new Date(t.timestamp).toLocaleTimeString('en-GB', { hour12: false })}</td>
              <td className={c}>{fmt(t.price)}</td>
              <td>{t.volume.toLocaleString()}</td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}

function Flow({ symbol }: { symbol: string }) {
  const [rows, setRows] = useState<FlowRecord[] | null>(null)
  const [error, setError] = useState('')
  useEffect(() => {
    setRows(null)
    setError('')
    marketApi.investorFlow(symbol, 20).then((r) => setRows(r.records)).catch((e) => setError(e.message))
  }, [symbol])
  if (error) return <p className="muted small">{error}</p>
  if (!rows) return <p className="muted small">불러오는 중…</p>
  const v = (n: number | null) => (n === null ? '-' : `${n > 0 ? '+' : ''}${compact(n)}`)
  const c = (n: number | null) => (!n ? '' : n > 0 ? 'up' : 'down')
  const sum = (k: 'individual' | 'foreigner' | 'institution', days: number) => rows.slice(0, days).reduce((a, r) => a + (r[k] ?? 0), 0)
  return (
    <>
      <p className="muted small">순매수 수량 (주). 빨강 = 순매수, 파랑 = 순매도</p>
      <table className="ticks flow">
        <thead>
          <tr><th>날짜</th><th>개인</th><th>외국인</th><th>기관</th></tr>
        </thead>
        <tbody>
          {[5, 20].map((d) => (
            <tr key={d} className="flow-sum">
              <td>{d}일 합계</td>
              {(['individual', 'foreigner', 'institution'] as const).map((k) => <td key={k} className={c(sum(k, d))}>{v(sum(k, d))}</td>)}
            </tr>
          ))}
          {rows.map((r) => (
            <tr key={r.date}>
              <td className="muted">{r.date.slice(5)}</td>
              <td className={c(r.individual)}>{v(r.individual)}</td>
              <td className={c(r.foreigner)}>{v(r.foreigner)}</td>
              <td className={c(r.institution)}>{v(r.institution)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {rows[0]?.foreign_holding_pct != null && (
        <p className="small">외국인 보유율 <b>{rows[0].foreign_holding_pct.toFixed(2)}%</b></p>
      )}
    </>
  )
}

interface Props {
  market: Market
  symbol: string
  book: OrderBook | null
  ticks: Tick[]
  state: StreamState
  prevClose: number | null
}

/** Right column: live orderbook + ticks, and investor flows (KR). */
export default function SidePanel({ market, symbol, book, ticks, state, prevClose }: Props) {
  const [tab, setTab] = useState<'book' | 'flow'>('book')
  return (
    <aside className="side-panel">
      <div className="tabs">
        <button className={tab === 'book' ? 'active' : ''} onClick={() => setTab('book')}>호가·체결</button>
        <button className={tab === 'flow' ? 'active' : ''} onClick={() => setTab('flow')}>투자자 수급</button>
        <div className="spacer" />
        {tab === 'book' && <span className={`stream-state ${state}`}><i className="dot" aria-hidden />{STATE_LABEL[state]}</span>}
      </div>
      {tab === 'book' && (
        <>
          {book ? <Book book={book} prevClose={prevClose} market={market} /> : <p className="muted small">{state === 'unavailable' ? '토스증권 연동 시 제공됩니다' : '불러오는 중…'}</p>}
          {ticks.length > 0 && <Ticks ticks={ticks} market={market} />}
        </>
      )}
      {tab === 'flow' && (market === 'KR' ? <Flow symbol={symbol} /> : <p className="muted small">투자자별 수급은 국내 종목만 제공됩니다.</p>)}
    </aside>
  )
}
