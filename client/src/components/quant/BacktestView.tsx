import { useMemo } from 'react'
import { useNavigate } from 'react-router-dom'
import { MOBILE, useMediaQuery } from '../../hooks/useMediaQuery'
import { PREFILL_KEY } from '../../api/autotrade'
import type { BacktestResult } from '../../api/quant'
import { priceFormatter } from '../chart/format'
import ChartDiagnosis from '../chart/ChartDiagnosis'
import TradingChart, { type ChartMarker } from '../chart/TradingChart'
import { DOWN, UP, sanitize } from '../../indicators/registry'
import { investingUrl } from '../../lib/externalLinks'
import EquityChart from './EquityChart'
import MetricsTable from './MetricsTable'
import TradesTable from './TradesTable'

export const day = (t: number | null) => (t ? new Date(t * 1000).toISOString().slice(0, 10) : '-')

export default function BacktestView({ result, title }: { result: BacktestResult; title: string }) {
  const isMobile = useMediaQuery(MOBILE)
  const money = priceFormatter(result.market)
  const markers = useMemo<ChartMarker[]>(() => {
    const withText = result.trades.length <= (isMobile ? 6 : 20) // labels only when they won't overlap
    return result.trades.flatMap((t) => [
      { time: t.entry_time, position: 'belowBar' as const, shape: 'arrowUp' as const, color: UP, text: withText ? '매수' : undefined },
      ...(t.exit_time
        ? [{ time: t.exit_time, position: 'aboveBar' as const, shape: 'arrowDown' as const, color: DOWN, text: withText ? '매도' : undefined }]
        : []),
    ])
  }, [result, isMobile])
  const indicators = useMemo(() => sanitize(result.chart_indicators), [result])
  const navigate = useNavigate()
  const makeBot = () => {
    sessionStorage.setItem(PREFILL_KEY, JSON.stringify({ market: result.market, symbol: result.symbol, strategy: result.strategy, params: result.params }))
    navigate('/auto')
  }

  return (
    <>
      <div className="result-head">
        <div className="account-head">
          <h3>{title}</h3>
          <a className="ext-link" href={investingUrl(result.market, result.symbol)} target="_blank" rel="noopener noreferrer" title="Investing.com에서 종목 정보 보기">
            Investing ↗
          </a>
          <div className="spacer" />
          <button
            className="primary"
            onClick={makeBot}
            title="지금 검증한 종목·전략·파라미터 그대로 자동매매 봇 생성 창을 엽니다 (모의 모드·꺼진 상태로 안전하게 시작)"
          >
            이 설정으로 봇 만들기 →
          </button>
        </div>
        <span className="muted">
          {day(result.period.start)} ~ {day(result.period.end)} · 초기 자본 {money(result.capital)} {result.currency} ·{' '}
          {Object.entries(result.params).map(([k, v]) => `${k}=${v}`).join(', ')}
        </span>
      </div>
      {result.warnings.map((w) => (
        <p key={w} className="warn">{w}</p>
      ))}
      <div className="result-grid">
        <MetricsTable strategy={result.metrics} benchmark={result.benchmark} market={result.market} interval={result.interval} />
        <EquityChart points={result.equity} market={result.market} />
      </div>
      <ChartDiagnosis
        candles={result.candles}
        market={result.market}
        interval={result.interval ?? '1d'}
        activeIndicators={indicators}
      />
      <div className="quant-price-chart">
        <TradingChart
          candles={result.candles}
          market={result.market}
          interval={result.interval ?? '1d'}
          indicators={indicators}
          viewKey={`${result.symbol}:${result.strategy}:${result.period.start}:${JSON.stringify(result.params)}`}
          markers={markers}
          visibleFrom={result.start_index}
          logScale
        />
      </div>
      <TradesTable trades={result.trades} market={result.market} interval={result.interval} />
    </>
  )
}
