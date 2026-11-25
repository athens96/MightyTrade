import { useEffect, useState } from 'react'
import { marketApi, type IndexQuote, type MarketRegime, type SessionStatus } from '../../api/market'

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

function RegimePill({ r }: { r?: MarketRegime }) {
  if (!r) return null
  const tip = `${r.benchmark_name ?? r.benchmark} 종가 ${r.close ?? '-'} · ${r.ma_short}일선 ${r.ma_short_val ?? '-'} / ${r.ma_long}일선 ${r.ma_long_val ?? '-'}`
  return (
    <span className="mb-status" title={tip}>
      <span className="muted">{r.market === 'KR' ? '국내온도' : '미국온도'}</span> <b>{r.label}</b>
    </span>
  )
}

/** KOSPI / KOSDAQ, Market Timing traffic lights, and KR / US session status. */
export default function MarketBar() {
  const [data, setData] = useState<{ indices: IndexQuote[]; status: Record<'KR' | 'US', SessionStatus> } | null>(null)
  const [regimes, setRegimes] = useState<{ KR: MarketRegime; US: MarketRegime } | null>(null)
  useEffect(() => {
    let alive = true
    let lastOv = 0
    const loadOverview = (force = false) => {
      if (!force && Date.now() - lastOv < 10_000) return
      lastOv = Date.now()
      marketApi.overview().then((d) => alive && setData(d)).catch(() => undefined)
    }
    const loadRegimes = () => {
      marketApi.regimes().then((r) => alive && setRegimes(r)).catch(() => undefined)
    }
    loadOverview(true)
    loadRegimes()
    const onVisible = () => {
      if (document.visibilityState === 'visible') loadOverview()
    }
    document.addEventListener('visibilitychange', onVisible)
    const tOv = window.setInterval(() => document.visibilityState === 'visible' && loadOverview(), 30_000)
    const tReg = window.setInterval(() => document.visibilityState === 'visible' && loadRegimes(), 300_000)
    return () => {
      alive = false
      document.removeEventListener('visibilitychange', onVisible)
      window.clearInterval(tOv)
      window.clearInterval(tReg)
    }
  }, [])
  if (!data && !regimes) return null
  return (
    <div className="market-bar">
      {data?.indices.map((i) => (
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
      <RegimePill r={regimes?.KR} />
      <RegimePill r={regimes?.US} />
      <div className="spacer" />
      {data && <Status name="국내" s={data.status.KR} />}
      {data && <Status name="미국" s={data.status.US} />}
    </div>
  )
}
