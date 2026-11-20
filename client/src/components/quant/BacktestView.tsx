import { useMemo } from 'react'
import type { BacktestResult } from '../../api/quant'
import { priceFormatter } from '../chart/format'
import TradingChart, { type ChartMarker } from '../chart/TradingChart'
import { DOWN, UP, sanitize } from '../../indicators/registry'
import EquityChart from './EquityChart'
import MetricsTable from './MetricsTable'
import TradesTable from './TradesTable'

export const day = (t: number | null) => (t ? new Date(t * 1000).toISOString().slice(0, 10) : '-')

export default function BacktestView({ result, title }: { result: BacktestResult; title: string }) {
  const money = priceFormatter(result.market)
  const markers = useMemo<ChartMarker[]>(() => {
    const withText = result.trades.length <= 20 // labels only when they won't overlap
    return result.trades.flatMap((t) => [
      { time: t.entry_time, position: 'belowBar' as const, shape: 'arrowUp' as const, color: UP, text: withText ? '매수' : undefined },
      ...(t.exit_time
        ? [{ time: t.exit_time, position: 'aboveBar' as const, shape: 'arrowDown' as const, color: DOWN, text: withText ? '매도' : undefined }]
        : []),
    ])
  }, [result])
  const indicators = useMemo(() => sanitize(result.chart_indicators), [result])

  return (
    <>
      <div className="result-head">
        <h3>{title}</h3>
        <span className="muted">
          {day(result.period.start)} ~ {day(result.period.end)} · 초기 자본 {money(result.capital)} {result.currency} ·{' '}
          {Object.entries(result.params).map(([k, v]) => `${k}=${v}`).join(', ')}
        </span>
      </div>
      {result.warnings.map((w) => (
        <p key={w} className="warn">{w}</p>
      ))}
      <div className="result-grid">
        <MetricsTable strategy={result.metrics} benchmark={result.benchmark} market={result.market} />
        <EquityChart points={result.equity} market={result.market} />
      </div>
      <div className="quant-price-chart">
        <TradingChart
          candles={result.candles}
          market={result.market}
          interval="1d"
          indicators={indicators}
          viewKey={`${result.symbol}:${result.strategy}:${result.period.start}:${JSON.stringify(result.params)}`}
          markers={markers}
          visibleFrom={result.start_index}
          logScale
        />
      </div>
      <TradesTable trades={result.trades} market={result.market} />
    </>
  )
}
