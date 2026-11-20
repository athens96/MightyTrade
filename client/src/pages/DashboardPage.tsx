import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { ApiError } from '../api/client'
import { marketApi, type StockWarning } from '../api/market'
import MarketBar from '../components/dashboard/MarketBar'
import SidePanel from '../components/dashboard/SidePanel'
import { useRealtime } from '../hooks/useRealtime'
import type { Candle, Instrument, Interval, Market, Quote, WatchItem, WatchQuote } from '../api/types'
import { priceFormatter } from '../components/chart/format'
import { settingsApi, type Settings } from '../api/settings'
import IndicatorPanel from '../components/chart/IndicatorPanel'
import TradingChart from '../components/chart/TradingChart'
import { DEFAULT_INDICATORS, sanitize, type IndicatorConfig } from '../indicators/registry'

const INTERVALS: Interval[] = ['1m', '5m', '15m', '1h', '1d', '1w']
const REFRESH_MS = 15_000
// Enough history for long indicators (MA120, Ichimoku 52+26) while showing ~150 bars.
const CHART_LIMIT = 500

export default function DashboardPage() {
  const [market, setMarket] = useState<Market>('KR')
  const [watchlist, setWatchlist] = useState<WatchItem[]>([])
  const [selected, setSelected] = useState<WatchItem | null>(null)
  const [interval, setChartInterval] = useState<Interval>('1d')
  const [candles, setCandles] = useState<Candle[]>([])
  const [quote, setQuote] = useState<Quote | null>(null)
  const [warnings, setWarnings] = useState<StockWarning[]>([])
  const [provider, setProvider] = useState('')
  const [error, setError] = useState('')
  const [needsKey, setNeedsKey] = useState(false)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<Instrument[]>([])
  const [searching, setSearching] = useState(false)
  const [indicators, setIndicators] = useState<IndicatorConfig[]>(DEFAULT_INDICATORS)
  const settingsRef = useRef<Settings | null>(null)
  const saveTimer = useRef<number | undefined>(undefined)

  // Indicator selection is stored per account in /users/me/settings under chart.indicators.
  useEffect(() => {
    settingsApi
      .get()
      .then(({ settings }) => {
        settingsRef.current = settings
        const chart = (settings.chart ?? {}) as { indicators?: unknown }
        if (chart.indicators !== undefined) setIndicators(sanitize(chart.indicators))
      })
      .catch(() => (settingsRef.current = {}))
  }, [])

  const changeIndicators = (next: IndicatorConfig[]) => {
    setIndicators(next)
    window.clearTimeout(saveTimer.current)
    saveTimer.current = window.setTimeout(() => {
      const base = settingsRef.current ?? {}
      const merged = { ...base, chart: { ...((base.chart as object) ?? {}), indicators: next } }
      settingsRef.current = merged
      settingsApi.put(merged).catch((e) => setError(e.message))
    }, 600)
  }

  useEffect(() => {
    marketApi.watchlist().then(({ items }) => setWatchlist(items)).catch((e) => setError(e.message))
  }, [])

  const items = watchlist.filter((w) => w.market === market)
  const [quotes, setQuotes] = useState<Record<string, WatchQuote>>({})
  const symbolsKey = items.map((w) => w.symbol).join(',')

  // Prices for every watchlist row in one request, refreshed while the tab is visible.
  useEffect(() => {
    if (!symbolsKey) return
    let cancelled = false
    const load = () =>
      marketApi
        .quotes(market, symbolsKey.split(','))
        .then(({ items }) => !cancelled && setQuotes((prev) => ({ ...prev, ...Object.fromEntries(items.map((q) => [q.symbol, q])) })))
        .catch(() => undefined) // the row just keeps its last price; the chart area shows real errors
    load()
    const timer = window.setInterval(() => document.visibilityState === 'visible' && load(), REFRESH_MS)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [market, symbolsKey])

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
        marketApi.candles(selected.market, selected.symbol, interval, CHART_LIMIT),
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

  // Live orderbook/ticks for the selected symbol; the latest tick also drives the header price.
  const live = useRealtime(selected?.market, selected?.symbol)
  const prevClose = quote ? quote.price - quote.change : null
  const lastTick = live.ticks[0]
  const shown = quote && lastTick && prevClose
    ? { ...quote, price: lastTick.price, change: lastTick.price - prevClose, change_pct: (lastTick.price / prevClose - 1) * 100 }
    : quote

  useEffect(() => {
    setWarnings([])
    if (!selected) return
    marketApi.warnings(selected.market, selected.symbol).then((r) => setWarnings(r.items)).catch(() => undefined)
  }, [selected])

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
              <div className="wl-left">
                <span className="wl-name">{w.name}</span>
                <span className="muted small">
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
              </div>
              <WatchPrice q={quotes[w.symbol]} market={market} />
            </li>
          ))}
        </ul>
      </aside>

      <section className="chart-panel">
        <MarketBar />
        <div className="chart-header">
          <h2>{selected ? `${selected.name} (${selected.symbol})` : '종목을 선택하세요'}</h2>
          {shown && (
            <span className={shown.change >= 0 ? 'up' : 'down'}>
              {fmt(shown.price)} {shown.currency} ({shown.change >= 0 ? '+' : ''}
              {fmt(shown.change)}, {shown.change >= 0 ? '+' : ''}
              {fmt(shown.change_pct)}%)
            </span>
          )}
          {warnings.map((w) => (
            <span key={w.type} className={`badge-warn ${w.level}`} title={w.end ? `${w.start ?? ''} ~ ${w.end}` : w.start ?? ''}>
              ⚠ {w.label}
            </span>
          ))}
          <div className="spacer" />
          <IndicatorPanel value={indicators} onChange={changeIndicators} />
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
        <TradingChart
          candles={candles}
          market={selected?.market ?? market}
          interval={interval}
          indicators={indicators}
          viewKey={`${selected?.market}:${selected?.symbol}:${interval}`}
        />
        <p className="muted small">data: {provider}</p>
      </section>
      {selected && (
        <SidePanel market={selected.market} symbol={selected.symbol} name={selected.name} lastPrice={shown?.price ?? null} book={live.book} ticks={live.ticks} state={live.state} prevClose={prevClose} />
      )}
    </div>
  )
}

function WatchPrice({ q, market }: { q?: WatchQuote; market: Market }) {
  if (!q) return <div className="wl-right muted small">…</div>
  if (q.error || q.price === undefined) return <div className="wl-right muted small" title={q.error}>-</div>
  const fmt = priceFormatter(market)
  const ch = q.change ?? 0
  const cls = ch > 0 ? 'up' : ch < 0 ? 'down' : 'muted'
  const sign = ch > 0 ? '+' : ''
  return (
    <div className="wl-right">
      <span className="wl-price">{fmt(q.price)}</span>
      <span className={`small ${cls}`}>
        {sign}
        {fmt(ch)} {sign}
        {(q.change_pct ?? 0).toFixed(2)}%
      </span>
    </div>
  )
}
