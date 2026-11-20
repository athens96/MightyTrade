import { useEffect, useRef } from 'react'
import {
  CandlestickSeries,
  HistogramSeries,
  createChart,
  type IChartApi,
  type ISeriesApi,
  type UTCTimestamp,
} from 'lightweight-charts'
import type { Candle } from '../api/types'

// Korean convention: up = red, down = blue.
const UP = '#e5484d'
const DOWN = '#3e63dd'

// lightweight-charts renders timestamps as UTC. Intraday bars are shifted to the browser's local
// time; daily/weekly bars come from the server as UTC midnight of the trading date and stay as-is.
const toLocal = (t: number) => (t - new Date(t * 1000).getTimezoneOffset() * 60) as UTCTimestamp

export default function CandleChart({ candles, intraday }: { candles: Candle[]; intraday: boolean }) {
  const containerRef = useRef<HTMLDivElement>(null)
  const chartRef = useRef<IChartApi | null>(null)
  const candleRef = useRef<ISeriesApi<'Candlestick'> | null>(null)
  const volumeRef = useRef<ISeriesApi<'Histogram'> | null>(null)

  useEffect(() => {
    const el = containerRef.current!
    const chart = createChart(el, {
      autoSize: true,
      layout: { background: { color: '#111418' }, textColor: '#c9d1d9' },
      grid: { vertLines: { color: '#1f242b' }, horzLines: { color: '#1f242b' } },
      timeScale: { timeVisible: true },
    })
    candleRef.current = chart.addSeries(CandlestickSeries, {
      upColor: UP, downColor: DOWN, borderVisible: false, wickUpColor: UP, wickDownColor: DOWN,
    })
    volumeRef.current = chart.addSeries(HistogramSeries, { priceScaleId: 'vol', priceFormat: { type: 'volume' } })
    chart.priceScale('vol').applyOptions({ scaleMargins: { top: 0.8, bottom: 0 } })
    chartRef.current = chart
    return () => chart.remove()
  }, [])

  useEffect(() => {
    const time = (t: number) => (intraday ? toLocal(t) : (t as UTCTimestamp))
    candleRef.current?.setData(
      candles.map((c) => ({ time: time(c.time), open: c.open, high: c.high, low: c.low, close: c.close })),
    )
    volumeRef.current?.setData(
      candles.map((c) => ({ time: time(c.time), value: c.volume, color: c.close >= c.open ? UP + '80' : DOWN + '80' })),
    )
    chartRef.current?.applyOptions({ timeScale: { timeVisible: intraday } })
    chartRef.current?.timeScale().fitContent()
  }, [candles, intraday])

  return <div ref={containerRef} className="chart" />
}
