import { useEffect, useRef, useState } from 'react'
import {
  BaselineSeries,
  CrosshairMode,
  LineSeries,
  PriceScaleMode,
  createChart,
  type IChartApi,
  type ISeriesApi,
  type MouseEventParams,
  type Time,
  type UTCTimestamp,
} from 'lightweight-charts'
import type { Market } from '../../api/types'
import type { EquityPoint } from '../../api/quant'
import { priceFormatter } from '../chart/format'

const STRATEGY = '#4fc3f7'
const BENCH = '#8b949e'

/** Strategy equity vs buy & hold (top) and drawdown % (bottom pane). */
export default function EquityChart({ points, market }: { points: EquityPoint[]; market: Market }) {
  const ref = useRef<HTMLDivElement>(null)
  const chartRef = useRef<IChartApi | null>(null)
  const series = useRef<{ eq: ISeriesApi<'Line'>; bench: ISeriesApi<'Line'>; dd: ISeriesApi<'Baseline'> } | null>(null)
  const [hover, setHover] = useState<number | null>(null)
  const fmt = priceFormatter(market)

  useEffect(() => {
    const chart = createChart(ref.current!, {
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
      timeScale: { borderColor: '#2a2f36' },
    })
    const priceFormat = { type: 'custom' as const, formatter: fmt, minMove: market === 'KR' ? 1 : 0.01 }
    series.current = {
      bench: chart.addSeries(LineSeries, { color: BENCH, lineWidth: 1, priceFormat, priceLineVisible: false, title: '단순 보유' }),
      eq: chart.addSeries(LineSeries, { color: STRATEGY, lineWidth: 2, priceFormat, priceLineVisible: false, title: '전략' }),
      dd: chart.addSeries(
        BaselineSeries,
        {
          baseValue: { type: 'price', price: 0 },
          topLineColor: 'transparent',
          topFillColor1: 'transparent',
          topFillColor2: 'transparent',
          bottomLineColor: '#e5484d',
          bottomFillColor1: 'rgba(229,72,77,0.05)',
          bottomFillColor2: 'rgba(229,72,77,0.35)',
          lineWidth: 1,
          priceFormat: { type: 'custom', formatter: (v: number) => `${v.toFixed(1)}%`, minMove: 0.01 },
          priceLineVisible: false,
          title: '낙폭',
        },
        1,
      ),
    }
    // log scale: equal % moves look equal across a long period
    chart.priceScale('right', 0).applyOptions({ mode: PriceScaleMode.Logarithmic })
    chart.panes()[0]?.setStretchFactor(3)
    chart.panes()[1]?.setStretchFactor(1)
    const onMove = (p: MouseEventParams<Time>) => setHover(p.logical === undefined ? null : Math.round(p.logical))
    chart.subscribeCrosshairMove(onMove)
    chartRef.current = chart
    return () => {
      chart.unsubscribeCrosshairMove(onMove)
      chart.remove()
      chartRef.current = null
      series.current = null
    }
  }, [market])

  useEffect(() => {
    const s = series.current
    if (!s) return
    const t = (p: EquityPoint) => p.time as UTCTimestamp
    s.eq.setData(points.map((p) => ({ time: t(p), value: p.equity })))
    s.bench.setData(points.map((p) => ({ time: t(p), value: p.benchmark })))
    s.dd.setData(points.map((p) => ({ time: t(p), value: p.drawdown })))
    chartRef.current?.timeScale().fitContent()
  }, [points])

  const p = points[hover !== null && hover >= 0 && hover < points.length ? hover : points.length - 1]
  const date = p ? new Date(p.time * 1000).toISOString().slice(0, 10) : ''

  return (
    <div className="equity-chart">
      <div ref={ref} className="chart" />
      {p && (
        <div className="legend" style={{ top: 4 }}>
          <div className="legend-row">
            <span className="muted">{date}</span>
            <span style={{ color: STRATEGY }}>전략 {fmt(p.equity)}</span>
            <span style={{ color: BENCH }}>단순 보유 {fmt(p.benchmark)}</span>
            <span className="down">낙폭 {p.drawdown.toFixed(2)}%</span>
          </div>
        </div>
      )}
    </div>
  )
}
