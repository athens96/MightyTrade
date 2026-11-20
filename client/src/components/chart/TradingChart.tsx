import { useEffect, useMemo, useRef, useState } from 'react'
import {
  CandlestickSeries,
  CrosshairMode,
  HistogramSeries,
  LineSeries,
  LineStyle,
  PriceScaleMode,
  createChart,
  createSeriesMarkers,
  type ISeriesMarkersPluginApi,
  type IChartApi,
  type ISeriesApi,
  type MouseEventParams,
  type SeriesType,
  type Time,
  type UTCTimestamp,
} from 'lightweight-charts'
import type { Candle, Interval, Market } from '../../api/types'
import type { Bars } from '../../indicators/calc'
import {
  DOWN,
  INDICATOR_MAP,
  UP,
  type Band,
  type IndicatorConfig,
  type IndicatorDef,
  type IndicatorOutput,
  type Plot,
} from '../../indicators/registry'
import { BandFill } from './bandFill'
import { compact, priceFormatOption, priceFormatter, valueFormatter } from './format'

const VISIBLE_BARS = 150
const DAY = 86400
const STEP: Record<Interval, number> = { '1m': 60, '5m': 300, '15m': 900, '1h': 3600, '1d': DAY, '1w': 7 * DAY }
const NO_PARAM_SUMMARY = new Set(['ma', 'ema', 'volume', 'vwap', 'obv'])

// lightweight-charts renders timestamps as UTC. Intraday bars are shifted to the browser's local time;
// daily/weekly bars arrive as UTC midnight of the trading date and are shown as-is.
const toLocal = (t: number) => t - new Date(t * 1000).getTimezoneOffset() * 60

/** Timestamps after the last bar, for lines plotted into the future (Ichimoku leading spans). */
function futureTimes(last: number, interval: Interval, count: number): number[] {
  const out: number[] = []
  let t = last
  while (out.length < count) {
    t += STEP[interval]
    const wd = new Date(t * 1000).getUTCDay()
    if (interval === '1d' && (wd === 0 || wd === 6)) continue
    out.push(t)
  }
  return out
}

interface Item {
  cfg: IndicatorConfig
  def: IndicatorDef
  out: IndicatorOutput
  pane: number
}

interface Built {
  key: string
  series: Map<string, ISeriesApi<SeriesType>>
  bands: { fill: BandFill; item: string; band: Band }[]
}

function plotPoints(plot: Plot, disp: number[]) {
  const offset = plot.offset ?? 0
  const pts: { time: UTCTimestamp; value: number; color?: string }[] = []
  plot.values.forEach((v, i) => {
    const j = i + offset
    if (v === null || j < 0 || j >= disp.length) return
    const color = plot.colors?.[i]
    pts.push(color ? { time: disp[j] as UTCTimestamp, value: v, color } : { time: disp[j] as UTCTimestamp, value: v })
  })
  return pts
}

function bandPoints(out: IndicatorOutput, band: Band, disp: number[]) {
  const upper = out.plots.find((p) => p.id === band.upper)
  const lower = out.plots.find((p) => p.id === band.lower)
  if (!upper || !lower) return []
  const offset = upper.offset ?? 0
  const pts: { time: UTCTimestamp; upper: number; lower: number }[] = []
  upper.values.forEach((u, i) => {
    const l = lower.values[i]
    const j = i + offset
    if (u === null || l === null || j < 0 || j >= disp.length) return
    pts.push({ time: disp[j] as UTCTimestamp, upper: u, lower: l })
  })
  return pts
}

export interface ChartMarker {
  time: number // raw bar time (same as candle.time)
  position: 'aboveBar' | 'belowBar'
  shape: 'arrowUp' | 'arrowDown' | 'circle'
  color: string
  text?: string
}

interface Props {
  candles: Candle[]
  market: Market
  interval: Interval
  indicators: IndicatorConfig[]
  viewKey: string // visible range resets when this changes (symbol / interval)
  markers?: ChartMarker[] // e.g. backtest buy/sell points
  visibleFrom?: number // bar index the initial view starts at (default: last ~150 bars)
  logScale?: boolean // logarithmic price axis (long backtests)
}

export default function TradingChart({ candles, market, interval, indicators, viewKey, markers, visibleFrom, logScale }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const chartRef = useRef<IChartApi | null>(null)
  const candleRef = useRef<ISeriesApi<'Candlestick'> | null>(null)
  const markersRef = useRef<ISeriesMarkersPluginApi<Time> | null>(null)
  const builtRef = useRef<Built>({ key: '', series: new Map(), bands: [] })
  const viewKeyRef = useRef('')
  const [hover, setHover] = useState<number | null>(null)
  const [paneTops, setPaneTops] = useState<number[]>([0])

  const intraday = interval !== '1d' && interval !== '1w'

  const computed = useMemo(() => {
    const bars: Bars = {
      time: candles.map((c) => c.time),
      open: candles.map((c) => c.open),
      high: candles.map((c) => c.high),
      low: candles.map((c) => c.low),
      close: candles.map((c) => c.close),
      volume: candles.map((c) => c.volume),
    }
    let pane = 1
    const items: Item[] = []
    for (const cfg of indicators) {
      const def = INDICATOR_MAP[cfg.id]
      if (!def) continue
      const out = def.compute(bars, cfg.params, { market, intraday })
      const paneIdx = def.group === 'overlay' ? 0 : out.plots.length ? pane++ : -1
      items.push({ cfg, def, out, pane: paneIdx })
    }
    const maxOffset = Math.max(0, ...items.flatMap((it) => it.out.plots.map((p) => p.offset ?? 0)))
    const raw = candles.length ? [...bars.time, ...futureTimes(bars.time[bars.time.length - 1], interval, maxOffset)] : []
    const disp = raw.map((t) => (intraday ? toLocal(t) : t))
    const indexOfTime = new Map(disp.map((t, i) => [t, i]))
    const key =
      market +
      '|' +
      items.map((it) => `${it.def.id}:${it.pane}:${it.out.plots.map((p) => p.id).join(',')}:${JSON.stringify(it.out.levels ?? [])}`).join('|')
    return { items, disp, indexOfTime, key }
  }, [candles, indicators, market, interval, intraday])

  const computedRef = useRef(computed)
  computedRef.current = computed

  const updatePaneTops = () => {
    const chart = chartRef.current
    const wrap = wrapRef.current
    if (!chart || !wrap) return
    const top = wrap.getBoundingClientRect().top
    const tops = chart.panes().map((p) => (p.getHTMLElement()?.getBoundingClientRect().top ?? top) - top)
    setPaneTops((prev) => (prev.length === tops.length && prev.every((v, i) => Math.abs(v - tops[i]) < 1) ? prev : tops))
  }

  // Create the chart once.
  useEffect(() => {
    const chart = createChart(containerRef.current!, {
      autoSize: true,
      layout: {
        background: { color: '#111418' },
        textColor: '#c9d1d9',
        panes: { separatorColor: '#2a2f36', separatorHoverColor: 'rgba(120,130,145,0.3)', enableResize: true },
      },
      grid: { vertLines: { color: '#1b2026' }, horzLines: { color: '#1b2026' } },
      crosshair: { mode: CrosshairMode.Normal },
      localization: { locale: 'ko-KR', dateFormat: 'yyyy-MM-dd' },
      rightPriceScale: { borderColor: '#2a2f36' },
      timeScale: { borderColor: '#2a2f36', rightOffset: 5 },
    })
    candleRef.current = chart.addSeries(CandlestickSeries, {
      upColor: UP,
      downColor: DOWN,
      borderVisible: false,
      wickUpColor: UP,
      wickDownColor: DOWN,
    })
    markersRef.current = createSeriesMarkers(candleRef.current, [])
    chartRef.current = chart

    const onMove = (param: MouseEventParams<Time>) => {
      const t = param.time as number | undefined
      const idx = t === undefined ? null : computedRef.current.indexOfTime.get(t) ?? null
      setHover((prev) => (prev === idx ? prev : idx))
      updatePaneTops()
    }
    chart.subscribeCrosshairMove(onMove)
    // Re-measure pane positions for the sub-pane legends after the chart has re-laid out.
    const ro = new ResizeObserver(() => {
      requestAnimationFrame(updatePaneTops)
      window.setTimeout(updatePaneTops, 100)
    })
    ro.observe(wrapRef.current!)
    return () => {
      ro.disconnect()
      chart.unsubscribeCrosshairMove(onMove)
      chart.remove()
      chartRef.current = null
      markersRef.current = null
      builtRef.current = { key: '', series: new Map(), bands: [] }
    }
  }, [])

  // Sync data + indicator series.
  useEffect(() => {
    const chart = chartRef.current
    const candleSeries = candleRef.current
    if (!chart || !candleSeries) return
    const { items, disp, key } = computed

    chart.applyOptions({ timeScale: { timeVisible: intraday, secondsVisible: false } })
    chart.priceScale('right', 0).applyOptions({ mode: logScale ? PriceScaleMode.Logarithmic : PriceScaleMode.Normal })
    candleSeries.applyOptions({ priceFormat: priceFormatOption('price', market) })
    candleSeries.setData(
      candles.map((c, i) => ({ time: disp[i] as UTCTimestamp, open: c.open, high: c.high, low: c.low, close: c.close })),
    )

    const built = builtRef.current
    if (built.key !== key) {
      // Structure changed (indicator added/removed, plot set changed): rebuild all indicator series.
      // Removing the last series of a pane removes the pane, so sub-panes are recreated in order.
      for (const s of built.series.values()) chart.removeSeries(s)
      built.series.clear()
      built.bands = []
      for (const it of items) {
        if (it.pane < 0) continue
        const priceFormat = priceFormatOption(it.out.format, market)
        let first: ISeriesApi<SeriesType> | null = null
        for (const plot of it.out.plots) {
          const common = { priceFormat, lastValueVisible: false, priceLineVisible: false }
          const s =
            plot.kind === 'histogram'
              ? chart.addSeries(HistogramSeries, { ...common, color: plot.color }, it.pane)
              : chart.addSeries(
                  LineSeries,
                  {
                    ...common,
                    color: plot.color,
                    lineWidth: plot.lineWidth ?? 1,
                    lineStyle: plot.dashed ? LineStyle.Dashed : LineStyle.Solid,
                    crosshairMarkerVisible: false,
                    ...(plot.kind === 'dots' ? { lineVisible: false, pointMarkersVisible: true, pointMarkersRadius: 1.5 } : {}),
                  },
                  it.pane,
                )
          built.series.set(`${it.def.id}:${plot.id}`, s)
          first ??= s
        }
        for (const lv of it.out.levels ?? []) {
          first?.createPriceLine({
            price: lv.value,
            color: lv.color ?? '#5c6370',
            lineWidth: 1,
            lineStyle: LineStyle.Dashed,
            axisLabelVisible: true,
            title: '',
          })
        }
        for (const band of it.out.bands ?? []) {
          const s = built.series.get(`${it.def.id}:${band.upper}`)
          if (!s) continue
          const fill = new BandFill(band.fillUp, band.fillDown)
          s.attachPrimitive(fill)
          built.bands.push({ fill, item: it.def.id, band })
        }
      }
      chart.panes().forEach((p, i) => p.setStretchFactor(i === 0 ? 3 : 1))
      built.key = key
      requestAnimationFrame(updatePaneTops)
    }

    for (const it of items) {
      for (const plot of it.out.plots) built.series.get(`${it.def.id}:${plot.id}`)?.setData(plotPoints(plot, disp))
    }
    for (const b of built.bands) {
      const it = items.find((x) => x.def.id === b.item)
      if (it) b.fill.setPoints(bandPoints(it.out, b.band, disp))
    }

    if (candles.length && viewKeyRef.current !== viewKey) {
      viewKeyRef.current = viewKey
      const n = candles.length
      chart.timeScale().setVisibleLogicalRange({ from: visibleFrom ?? Math.max(0, n - VISIBLE_BARS), to: n + 3 })
    }
  }, [computed, candles, market, intraday, viewKey, visibleFrom, logScale])

  useEffect(() => {
    const toDisp = (t: number) => (intraday ? toLocal(t) : t) as UTCTimestamp
    markersRef.current?.setMarkers(
      [...(markers ?? [])].sort((a, b) => a.time - b.time).map((m) => ({ ...m, time: toDisp(m.time) })),
    )
  }, [markers, intraday])

  // ------------------------------------------------------------------ legend
  const n = candles.length
  const idx = hover ?? n - 1
  const fmtPrice = priceFormatter(market)

  const timeLabel = (i: number) => {
    const raw = computed.disp[i]
    if (raw === undefined) return ''
    const d = new Date(raw * 1000)
    const p = (v: number) => String(v).padStart(2, '0')
    const date = `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())}`
    return intraday ? `${date} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}` : date
  }

  const candleRow = () => {
    if (idx < 0 || idx >= n) return idx >= n ? <span className="muted">{timeLabel(idx)}</span> : null
    const c = candles[idx]
    const prev = idx > 0 ? candles[idx - 1].close : c.open
    const cls = (v: number) => (v > prev ? 'up' : v < prev ? 'down' : '')
    const chg = c.close - prev
    const pct = prev ? (chg / prev) * 100 : 0
    return (
      <>
        <span className="muted">{timeLabel(idx)}</span>
        <span>시 <b className={cls(c.open)}>{fmtPrice(c.open)}</b></span>
        <span>고 <b className={cls(c.high)}>{fmtPrice(c.high)}</b></span>
        <span>저 <b className={cls(c.low)}>{fmtPrice(c.low)}</b></span>
        <span>종 <b className={cls(c.close)}>{fmtPrice(c.close)}</b></span>
        <span className={cls(c.close)}>
          {chg >= 0 ? '+' : ''}
          {fmtPrice(chg)} ({chg >= 0 ? '+' : ''}
          {pct.toFixed(2)}%)
        </span>
        <span>거래량 <b>{compact(c.volume)}</b></span>
      </>
    )
  }

  const indicatorRow = (it: Item) => {
    const fmt = valueFormatter(it.out.format, market)
    const params = it.def.params.map((p) => it.cfg.params[p.key])
    const title = NO_PARAM_SUMMARY.has(it.def.id) || !params.length ? it.def.short : `${it.def.short}(${params.join(', ')})`
    return (
      <div className="legend-row" key={it.def.id}>
        <span className="legend-name">{title}</span>
        {it.out.note && <span className="muted">{it.out.note}</span>}
        {it.out.plots.map((plot) => {
          const v = plot.values[idx - (plot.offset ?? 0)]
          return (
            <span key={plot.id} style={{ color: plot.kind === 'histogram' ? undefined : plot.color }}>
              {plot.label && <span className="muted">{plot.label} </span>}
              {v === null || v === undefined ? '-' : fmt(v)}
            </span>
          )
        })}
      </div>
    )
  }

  const paneItems = computed.items.filter((it) => it.pane > 0)

  return (
    <div className="chart-wrap" ref={wrapRef}>
      <div ref={containerRef} className="chart" />
      <div className="legend" style={{ top: (paneTops[0] ?? 0) + 4 }}>
        <div className="legend-row">{candleRow()}</div>
        {computed.items.filter((it) => it.pane === 0 || (it.def.group === 'overlay' && it.out.note)).map(indicatorRow)}
      </div>
      {paneItems.map((it) =>
        paneTops[it.pane] !== undefined ? (
          <div className="legend" key={it.def.id} style={{ top: paneTops[it.pane] + 4 }}>
            {indicatorRow(it)}
          </div>
        ) : null,
      )}
    </div>
  )
}
