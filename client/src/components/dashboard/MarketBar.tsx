import { useEffect, useState } from 'react'
import { marketApi, type IndexQuote, type SessionStatus } from '../../api/market'

const time = (iso?: string) => (iso ? new Date(iso).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit', hour12: false }) : '')

function Status({ name, s }: { name: string; s: SessionStatus }) {
  return (
    <span className="mb-status" title={s.open ? `${time(s.until)}까지` : s.next ? `다음: ${s.next.label} ${new Date(s.next.at).toLocaleString('ko-KR')}` : ''}>
      <i className={`dot ${s.open ? 'on' : ''}`} aria-hidden />
      {name} <b>{s.label}</b>
      <span className="muted">{s.open ? ` ~${time(s.until)}` : s.next ? ` · ${s.next.label} ${time(s.next.at)}` : ''}</span>
    </span>
  )
}

/** KOSPI / KOSDAQ and KR / US session status. Hidden when the market data source isn't Toss. */
export default function MarketBar() {
  const [data, setData] = useState<{ indices: IndexQuote[]; status: Record<'KR' | 'US', SessionStatus> } | null>(null)
  useEffect(() => {
    let alive = true
    const load = () => marketApi.overview().then((d) => alive && setData(d)).catch(() => undefined)
    load()
    const t = window.setInterval(() => document.visibilityState === 'visible' && load(), 30_000)
    return () => {
      alive = false
      window.clearInterval(t)
    }
  }, [])
  if (!data) return null
  return (
    <div className="market-bar">
      {data.indices.map((i) => (
        <span key={i.symbol} className="mb-index">
          <span className="muted">{i.symbol}</span>{' '}
          <b>{i.price.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}</b>{' '}
          <span className={i.change > 0 ? 'up' : i.change < 0 ? 'down' : 'muted'}>
            {i.change > 0 ? '+' : ''}
            {i.change.toFixed(2)} ({i.change > 0 ? '+' : ''}
            {i.change_pct.toFixed(2)}%)
          </span>
        </span>
      ))}
      <div className="spacer" />
      <Status name="국내" s={data.status.KR} />
      <Status name="미국" s={data.status.US} />
    </div>
  )
}
