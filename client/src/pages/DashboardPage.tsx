import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { ApiError } from '../api/client'
import { marketApi } from '../api/market'
import type { Candle, Instrument, Interval, Market, Quote, WatchItem } from '../api/types'
import CandleChart from '../components/CandleChart'

const INTERVALS: Interval[] = ['1m', '5m', '15m', '1h', '1d', '1w']
const REFRESH_MS = 15_000

export default function DashboardPage() {
  const [market, setMarket] = useState<Market>('KR')
  const [watchlist, setWatchlist] = useState<WatchItem[]>([])
  const [selected, setSelected] = useState<WatchItem | null>(null)
  const [interval, setChartInterval] = useState<Interval>('1d')
  const [candles, setCandles] = useState<Candle[]>([])
  const [quote, setQuote] = useState<Quote | null>(null)
  const [provider, setProvider] = useState('')
  const [error, setError] = useState('')
  const [needsKey, setNeedsKey] = useState(false)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<Instrument[]>([])
  const [searching, setSearching] = useState(false)

  useEffect(() => {
    marketApi.watchlist().then(({ items }) => setWatchlist(items)).catch((e) => setError(e.message))
  }, [])

  const items = watchlist.filter((w) => w.market === market)

  useEffect(() => {
    if (!selected || selected.market !== market) setSelected(items[0] ?? null)
    setQuery('')
  }, [market, watchlist])

  // Chart + quote, refreshed periodically while the tab is visible.
  useEffect(() => {
    if (!selected) return
    let cancelled = false
    const load = () =>
      Promise.all([
        marketApi.candles(selected.market, selected.symbol, interval),
        marketApi.quote(selected.market, selected.symbol),
      ])
        .then(([c, q]) => {
          if (cancelled) return
          setCandles(c.candles)
          setProvider(c.provider)
          setQuote(q.quote)
          setError('')
          setNeedsKey(false)
        })
        .catch((e) => {
          if (cancelled) return
          setNeedsKey(e instanceof ApiError && e.code === 'toss_credentials_required')
          setError(e.message)
        })
    setCandles([])
    setQuote(null)
    load()
    const timer = window.setInterval(() => document.visibilityState === 'visible' && load(), REFRESH_MS)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [selected, interval])

  // Debounced symbol search (KR: 코드/종목명, US: 티커/한글명).
  useEffect(() => {
    const q = query.trim()
    if (!q) return setResults([])
    let cancelled = false
    const timer = window.setTimeout(() => {
      setSearching(true)
      marketApi
        .search(market, q)
        .then(({ items }) => !cancelled && setResults(items))
        .catch((e) => !cancelled && setError(e.message))
        .finally(() => !cancelled && setSearching(false))
    }, 300)
    return () => {
      cancelled = true
      window.clearTimeout(timer)
    }
  }, [query, market])

  const saveWatchlist = (next: WatchItem[]) => {
    setWatchlist(next)
    marketApi.saveWatchlist(next).catch((e) => setError(e.message))
  }

  const addItem = (inst: Instrument) => {
    const item: WatchItem = { market: inst.market, symbol: inst.symbol, name: inst.name }
    if (!watchlist.some((w) => w.market === item.market && w.symbol === item.symbol)) {
      saveWatchlist([...watchlist, item])
    }
    setSelected(item)
    setQuery('')
  }

  const removeItem = (item: WatchItem) => {
    const next = watchlist.filter((w) => !(w.market === item.market && w.symbol === item.symbol))
    saveWatchlist(next)
    if (selected?.symbol === item.symbol) setSelected(next.find((w) => w.market === market) ?? null)
  }

  const fmt = (n: number) => n.toLocaleString(undefined, { maximumFractionDigits: 2 })

  return (
    <div className="dashboard">
      <aside className="sidebar">
        <div className="tabs">
          {(['KR', 'US'] as Market[]).map((m) => (
            <button key={m} className={m === market ? 'active' : ''} onClick={() => setMarket(m)}>
              {m === 'KR' ? '국내' : '미국'}
            </button>
          ))}
        </div>

        <div className="search">
          <input
            className="search-input"
            placeholder={market === 'KR' ? '종목명 / 코드 검색' : '티커 / 한글명 검색'}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query.trim() && (
            <ul className="search-results">
              {searching && results.length === 0 && <li className="muted">검색 중… (첫 검색은 몇 초 걸려요)</li>}
              {!searching && results.length === 0 && <li className="muted">결과 없음</li>}
              {results.map((r) => (
                <li key={r.symbol} onClick={() => addItem(r)}>
                  <span>{r.name}</span>
                  <span className="muted">{r.symbol} · {r.exchange}</span>
                </li>
              ))}
            </ul>
          )}
        </div>

        <ul className="watchlist">
          {items.map((w) => (
            <li
              key={w.symbol}
              className={selected?.symbol === w.symbol ? 'active' : ''}
              onClick={() => setSelected(w)}
            >
              <span>{w.name}</span>
              <span className="muted">
                {w.symbol}
                <button
                  className="remove"
                  title="관심종목에서 삭제"
                  onClick={(e) => {
                    e.stopPropagation()
                    removeItem(w)
                  }}
                >
                  ×
                </button>
              </span>
            </li>
          ))}
        </ul>
      </aside>

      <section className="chart-panel">
        <div className="chart-header">
          <h2>{selected ? `${selected.name} (${selected.symbol})` : '종목을 선택하세요'}</h2>
          {quote && (
            <span className={quote.change >= 0 ? 'up' : 'down'}>
              {fmt(quote.price)} {quote.currency} ({quote.change >= 0 ? '+' : ''}
              {fmt(quote.change)}, {quote.change >= 0 ? '+' : ''}
              {fmt(quote.change_pct)}%)
            </span>
          )}
          <div className="spacer" />
          <div className="tabs">
            {INTERVALS.map((i) => (
              <button key={i} className={i === interval ? 'active' : ''} onClick={() => setChartInterval(i)}>
                {i}
              </button>
            ))}
          </div>
        </div>
        {needsKey ? (
          <p className="error">
            토스증권 API 키가 등록되지 않았습니다. <Link to="/settings">설정에서 등록하기 →</Link>
          </p>
        ) : (
          error && <p className="error">{error}</p>
        )}
        <CandleChart candles={candles} intraday={interval !== '1d' && interval !== '1w'} />
        <p className="muted small">data: {provider}</p>
      </section>
    </div>
  )
}
