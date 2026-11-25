import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { ApiError } from '../api/client'
import { marketApi, type StockWarning } from '../api/market'
import MarketBar from '../components/dashboard/MarketBar'
import SidePanel from '../components/dashboard/SidePanel'
import WatchCandle from '../components/dashboard/WatchCandle'
import { useRealtime } from '../hooks/useRealtime'
import type { Candle, Instrument, Interval, Market, Quote, WatchItem, WatchQuote } from '../api/types'
import { priceFormatter } from '../components/chart/format'
import { settingsApi, type Settings } from '../api/settings'
import IndicatorPanel from '../components/chart/IndicatorPanel'
import ChartDiagnosis from '../components/chart/ChartDiagnosis'
import TradingChart from '../components/chart/TradingChart'
import { DEFAULT_INDICATORS, sanitize, type IndicatorConfig } from '../indicators/registry'
import { FeatureGuideBox } from '../components/GuideBox'
import { FEATURE_GUIDES } from '../lib/featureGuides'
import { investingUrl } from '../lib/externalLinks'

const INTERVALS: Interval[] = ['1m', '5m', '15m', '1h', '1d', '1w']
const INTERVAL_HINTS: Record<Interval, string> = {
  '1m': '1분봉: 1분마다 기둥 하나씩 그려 초단기 체결 흐름을 봅니다',
  '5m': '5분봉: 장중 단타 매매에서 가장 많이 보는 단기 흐름입니다',
  '15m': '15분봉: 장중 잔파도를 줄이고 당일 추세를 깔끔하게 봅니다',
  '1h': '1시간봉: 며칠간의 단기 흐름과 지지·저항을 확인합니다',
  '1d': '일봉: 하루 1봉 기준으로 스윙·중기 추세를 보는 기본 차트입니다',
  '1w': '주봉: 1주일 1봉 기준으로 몇 달~몇 년의 큰 흐름을 확인합니다',
}
const DAILY_CHART_REFRESH_MS = 60_000
const INTRADAY_CHART_REFRESH_MS = 15_000
const QUOTE_REFRESH_OPTIONS = [10_000, 20_000, 60_000, 0] as const
type RefreshMs = (typeof QUOTE_REFRESH_OPTIONS)[number]
const REFRESH_PREF_KEY = 'mt.dashboard.refreshMs'
const FOCUS_COOLDOWN_MS = 3_000
// Enough history for long indicators (MA120, Ichimoku 52+26) while showing ~150 bars.
const CHART_LIMIT = 500

function loadRefreshPref(): RefreshMs {
  try {
    const raw = Number(localStorage.getItem(REFRESH_PREF_KEY))
    if ((QUOTE_REFRESH_OPTIONS as readonly number[]).includes(raw)) return raw as RefreshMs
  } catch {
    // ignore storage errors
  }
  return 10_000
}

function fmtClock(ts: number | null): string {
  if (!ts) return ''
  return new Date(ts).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
}

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
  const [refreshMs, setRefreshMs] = useState<RefreshMs>(loadRefreshPref)
  const [lastQuoteAt, setLastQuoteAt] = useState<number | null>(null)
  const [refreshingQuotes, setRefreshingQuotes] = useState(false)
  const [quotesError, setQuotesError] = useState('')
  const settingsRef = useRef<Settings | null>(null)
  const saveTimer = useRef<number | undefined>(undefined)
  const quotesBusyRef = useRef(false)
  const lastQuoteFetchRef = useRef(0)
  const chartBusyRef = useRef(false)
  const lastChartFetchRef = useRef(0)
  const selectedRef = useRef<WatchItem | null>(null)
  selectedRef.current = selected

  const cycleRefreshMs = () => {
    setRefreshMs((cur) => {
      const idx = QUOTE_REFRESH_OPTIONS.indexOf(cur)
      const next = QUOTE_REFRESH_OPTIONS[(idx + 1) % QUOTE_REFRESH_OPTIONS.length]
      try {
        localStorage.setItem(REFRESH_PREF_KEY, String(next))
      } catch {
        // ignore
      }
      return next
    })
  }

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

  const fetchWatchQuotes = useCallback(
    async (force = false) => {
      if (!symbolsKey) return
      if (quotesBusyRef.current) return
      if (!force && Date.now() - lastQuoteFetchRef.current < FOCUS_COOLDOWN_MS) return
      quotesBusyRef.current = true
      lastQuoteFetchRef.current = Date.now()
      setRefreshingQuotes(true)
      try {
        const { items: rows } = await marketApi.quotes(market, symbolsKey.split(','))
        const now = Date.now()
        setQuotes((prev) => {
          const next = { ...prev }
          for (const q of rows) next[q.symbol] = q
          return next
        })
        setLastQuoteAt(now)
        setQuotesError('')
        // Also keep the selected symbol's header price synced on every watchlist poll.
        const sel = selectedRef.current
        if (sel && sel.market === market) {
          const hit = rows.find((r) => r.symbol === sel.symbol && !r.error && r.price !== undefined)
          if (hit && hit.price !== undefined) {
            setQuote((prev) => ({
              market: sel.market,
              symbol: sel.symbol,
              price: hit.price!,
              change: hit.change ?? prev?.change ?? 0,
              change_pct: hit.change_pct ?? prev?.change_pct ?? 0,
              currency: hit.currency ?? prev?.currency ?? (sel.market === 'KR' ? 'KRW' : 'USD'),
              time: hit.time ?? prev?.time ?? Math.floor(now / 1000),
              open: hit.open ?? prev?.open,
              high: hit.high ?? prev?.high,
              low: hit.low ?? prev?.low,
              prev_close: hit.prev_close ?? prev?.prev_close,
            }))
          }
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : '시세 불러오기 지연'
        setQuotesError(msg)
      } finally {
        quotesBusyRef.current = false
        setRefreshingQuotes(false)
      }
    },
    [market, symbolsKey],
  )

  // Prices + today's OHLC candle for every watchlist row, refreshed periodically and on tab return.
  useEffect(() => {
    if (!symbolsKey) return
    fetchWatchQuotes(true)
    const onVisible = () => {
      if (document.visibilityState === 'visible') fetchWatchQuotes()
    }
    const onFocus = () => fetchWatchQuotes()
    document.addEventListener('visibilitychange', onVisible)
    window.addEventListener('focus', onFocus)
    const timer =
      refreshMs > 0
        ? window.setInterval(() => {
            if (document.visibilityState === 'visible') fetchWatchQuotes()
          }, refreshMs)
        : undefined
    return () => {
      document.removeEventListener('visibilitychange', onVisible)
      window.removeEventListener('focus', onFocus)
      if (timer !== undefined) window.clearInterval(timer)
    }
  }, [symbolsKey, refreshMs, fetchWatchQuotes])

  useEffect(() => {
    if (!selected || selected.market !== market) setSelected(items[0] ?? null)
    setQuery('')
  }, [market, watchlist])

  // Chart + quote, refreshed periodically while the tab is visible and immediately on tab return.
  useEffect(() => {
    if (!selected) return
    let cancelled = false
    const load = (force = false) => {
      if (chartBusyRef.current) return
      if (!force && Date.now() - lastChartFetchRef.current < FOCUS_COOLDOWN_MS) return
      chartBusyRef.current = true
      lastChartFetchRef.current = Date.now()
      Promise.all([
        marketApi.candles(selected.market, selected.symbol, interval, CHART_LIMIT),
        marketApi.quote(selected.market, selected.symbol),
      ])
        .then(([c, q]) => {
          if (cancelled) return
          setCandles(c.candles)
          setProvider(c.provider)
          setQuote(q.quote)
          setQuotes((prev) => ({ ...prev, [selected.symbol]: q.quote }))
          setLastQuoteAt(Date.now())
          setError('')
          setNeedsKey(false)
        })
        .catch((e) => {
          if (cancelled) return
          setNeedsKey(e instanceof ApiError && e.code === 'toss_credentials_required')
          setError(e.message)
        })
        .finally(() => {
          chartBusyRef.current = false
        })
    }
    setCandles([])
    setQuote(null)
    load(true)
    const onVisible = () => {
      if (document.visibilityState === 'visible') load()
    }
    document.addEventListener('visibilitychange', onVisible)
    const baseChartMs = interval === '1d' || interval === '1w' ? DAILY_CHART_REFRESH_MS : INTRADAY_CHART_REFRESH_MS
    const chartMs = refreshMs > 0 ? Math.max(baseChartMs, refreshMs) : 0
    const timer =
      chartMs > 0
        ? window.setInterval(() => {
            if (document.visibilityState === 'visible') load()
          }, chartMs)
        : undefined
    return () => {
      cancelled = true
      chartBusyRef.current = false
      document.removeEventListener('visibilitychange', onVisible)
      if (timer !== undefined) window.clearInterval(timer)
    }
  }, [selected, interval, refreshMs])

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

  // Live orderbook/ticks for the selected symbol; the latest tick drives header, watchlist row, and last candle.
  const live = useRealtime(selected?.market, selected?.symbol)
  const prevClose = quote ? (quote.prev_close ?? quote.price - quote.change) : null
  const lastTick = live.ticks[0]
  const shown = useMemo<Quote | null>(() => {
    if (!quote) return null
    if (!lastTick || !prevClose) return quote
    const px = lastTick.price
    return {
      ...quote,
      price: px,
      change: px - prevClose,
      change_pct: (px / prevClose - 1) * 100,
      high: quote.high !== undefined ? Math.max(quote.high, px) : undefined,
      low: quote.low !== undefined ? Math.min(quote.low, px) : undefined,
    }
  }, [quote, lastTick, prevClose])

  // Reflect live selected price into the rightmost chart candle so the chart never feels frozen between polls.
  const liveCandles = useMemo(() => {
    if (!shown || candles.length === 0) return candles
    const last = candles[candles.length - 1]
    const px = shown.price
    if (last.close === px && last.high >= px && last.low <= px) return candles
    const patched: Candle = {
      ...last,
      close: px,
      high: Math.max(last.high, px),
      low: Math.min(last.low, px),
    }
    return [...candles.slice(0, -1), patched]
  }, [candles, shown])

  useEffect(() => {
    setWarnings([])
    if (!selected) return
    marketApi.warnings(selected.market, selected.symbol).then((r) => setWarnings(r.items)).catch(() => undefined)
  }, [selected])

  const refreshLabel = refreshMs > 0 ? `자동 ${refreshMs / 1000}초` : '자동 꺼짐'

  return (
    <div className="dashboard">
      <aside className="sidebar">
        <div className="tabs">
          {(['KR', 'US'] as Market[]).map((m) => (
            <button
              key={m}
              className={m === market ? 'active' : ''}
              title={m === 'KR' ? '국내(코스피·코스닥) 주식과 ETF 시세를 봅니다' : '미국(나스닥·뉴욕) 주식과 ETF 시세를 봅니다'}
              onClick={() => setMarket(m)}
            >
              {m === 'KR' ? '국내' : '미국'}
            </button>
          ))}
        </div>

        <div className="search">
          <input
            className="search-input"
            placeholder={market === 'KR' ? '종목명 / 코드 검색' : '티커 / 영문명 / 한글명 검색'}
            title="종목 이름, 6자리 코드(예: 005930), 또는 미국 티커·영문명을 입력해 관심종목에 추가합니다"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query.trim() && (
            <ul className="search-results">
              {searching && results.length === 0 && <li className="muted">검색 중… (첫 검색은 몇 초 걸려요)</li>}
              {!searching && results.length === 0 && <li className="muted">결과 없음</li>}
              {results.map((r) => (
                <li key={r.symbol} title="클릭하면 관심종목에 추가하고 바로 차트를 엽니다" onClick={() => addItem(r)}>
                  <span>
                    {r.name}
                    {r.english_name && r.english_name !== r.name && <span className="muted small"> · {r.english_name}</span>}
                  </span>
                  <span className="muted">{r.symbol} · {r.exchange}</span>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="wl-toolbar">
          <button
            type="button"
            className={`wl-live-btn ${refreshMs > 0 ? 'on' : ''}`}
            onClick={cycleRefreshMs}
            title="클릭하여 관심종목 시세 자동 갱신 주기를 변경합니다 (10초 → 20초 → 60초 → 꺼짐)"
          >
            <i className={`dot ${refreshMs > 0 ? 'on' : ''}`} aria-hidden />
            {refreshLabel}
          </button>
          <span className="muted small wl-updated" title="관심종목 시세가 마지막으로 갱신된 시각">
            {lastQuoteAt ? fmtClock(lastQuoteAt) : '불러오는 중'}
          </span>
          <button
            type="button"
            className={`wl-refresh-btn ${refreshingQuotes ? 'spin' : ''}`}
            onClick={() => fetchWatchQuotes(true)}
            disabled={refreshingQuotes}
            title="지금 바로 관심종목 시세와 캔들을 새로 불러옵니다"
            aria-label="시세 즉시 새로고침"
          >
            ↻
          </button>
        </div>
        {quotesError && (
          <p className="small warn" style={{ marginTop: 6 }} title={quotesError}>
            시세 갱신 지연: {quotesError}
          </p>
        )}

        <ul className="watchlist">
          {items.map((w) => {
            const rowQuote: WatchQuote | undefined =
              selected?.market === w.market && selected?.symbol === w.symbol && shown
                ? shown
                : quotes[w.symbol]
            return (
              <li
                key={w.symbol}
                className={selected?.symbol === w.symbol ? 'active' : ''}
                title={`${w.name} (${w.symbol}) 차트와 실시간 호가 열기`}
                onClick={() => setSelected(w)}
              >
                <WatchCandle q={rowQuote} market={market} />
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
                <WatchPrice q={rowQuote} market={market} />
              </li>
            )
          })}
        </ul>
      </aside>

      <section className="chart-panel">
        <MarketBar />
        <div className="chart-header">
          <h2>{selected ? `${selected.name} (${selected.symbol})` : '종목을 선택하세요'}</h2>
          {selected && (
            <a className="ext-link" href={investingUrl(selected.market, selected.symbol)} target="_blank" rel="noopener noreferrer" title="Investing.com에서 종목 정보 보기">
              Investing ↗
            </a>
          )}
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
              <button key={i} className={i === interval ? 'active' : ''} title={INTERVAL_HINTS[i]} onClick={() => setChartInterval(i)}>
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
        <ChartDiagnosis
          candles={liveCandles}
          market={selected?.market ?? market}
          interval={interval}
          livePrice={shown?.price ?? null}
          activeIndicators={indicators}
        />
        <TradingChart
          candles={liveCandles}
          market={selected?.market ?? market}
          interval={interval}
          indicators={indicators}
          viewKey={`${selected?.market}:${selected?.symbol}:${interval}`}
        />
        <p className="muted small">data: {provider}</p>
        <FeatureGuideBox guide={FEATURE_GUIDES.dashboard} />
      </section>
      {selected && (
        <SidePanel market={selected.market} symbol={selected.symbol} name={selected.name} lastPrice={shown?.price ?? null} book={live.book} ticks={live.ticks} state={live.state} prevClose={prevClose} />
      )}
    </div>
  )
}

function WatchPrice({ q, market }: { q?: WatchQuote; market: Market }) {
  const prevPriceRef = useRef<number | undefined>(undefined)
  const [flash, setFlash] = useState<'up' | 'down' | ''>('')

  useEffect(() => {
    const px = q?.price
    if (px === undefined || q?.error) return
    const prev = prevPriceRef.current
    prevPriceRef.current = px
    if (prev !== undefined && px !== prev) {
      setFlash(px > prev ? 'up' : 'down')
      const t = window.setTimeout(() => setFlash(''), 650)
      return () => window.clearTimeout(t)
    }
  }, [q?.price, q?.error])

  if (!q) return <div className="wl-right muted small">…</div>
  if (q.error || q.price === undefined) return <div className="wl-right muted small" title={q.error}>-</div>
  const fmt = priceFormatter(market)
  const ch = q.change ?? 0
  const cls = ch > 0 ? 'up' : ch < 0 ? 'down' : 'muted'
  const sign = ch > 0 ? '+' : ''
  return (
    <div className={`wl-right${flash ? ` wl-flash-${flash}` : ''}`}>
      <span className="wl-price">{fmt(q.price)}</span>
      <span className={`small ${cls}`}>
        {sign}
        {fmt(ch)} {sign}
        {(q.change_pct ?? 0).toFixed(2)}%
      </span>
    </div>
  )
}
