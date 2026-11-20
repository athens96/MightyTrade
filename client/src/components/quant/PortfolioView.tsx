import { useNavigate } from 'react-router-dom'
import { PREFILL_KEY } from '../../api/autotrade'
import type { PortfolioBacktestResult, PortfolioModelMeta } from '../../api/quant'
import { investingUrl } from '../../lib/externalLinks'
import { priceFormatter } from '../chart/format'
import { day } from './BacktestView'
import EquityChart from './EquityChart'
import MetricsTable from './MetricsTable'

const REBAL_LABEL: Record<string, string> = {
  monthly: '매월 초',
  quarterly: '분기 초',
  yearly: '매년 초',
  drift: '비중 편차 이탈 시',
}

export default function PortfolioView({ result, model }: { result: PortfolioBacktestResult; model?: PortfolioModelMeta }) {
  const navigate = useNavigate()
  const money = priceFormatter(result.market)
  const nameOf = (sym: string) => result.assets.find((a) => a.symbol === sym)?.name || sym

  const makeBot = () => {
    sessionStorage.setItem(
      PREFILL_KEY,
      JSON.stringify({
        kind: 'rebalance',
        market: result.market,
        model: result.model,
        assets: result.assets,
        params: result.params,
        rebalance: result.rebalance,
        drift_pct: result.drift_pct,
        capital: result.capital,
      }),
    )
    navigate('/auto')
  }

  const weights = Object.entries(result.current_weights).sort((a, b) => b[1] - a[1])

  return (
    <>
      <div className="result-head">
        <div className="account-head">
          <h3>{model?.name ?? result.model} · 포트폴리오 백테스트</h3>
          <div className="spacer" />
          <button className="primary" onClick={makeBot} title="모의 모드로 리밸런싱 봇을 만듭니다">
            이 포트폴리오로 리밸런싱 봇 만들기 →
          </button>
        </div>
        <span className="muted">
          {day(result.period.start)} ~ {day(result.period.end)} · 초기 자본 {money(result.capital)} {result.currency} ·{' '}
          리밸런싱: {REBAL_LABEL[result.rebalance] ?? result.rebalance}
          {result.rebalance === 'drift' ? ` (±${result.drift_pct}%)` : ''} · 총 {result.metrics.rebalances}회 리밸런싱
        </span>
      </div>
      {result.warnings.map((w) => (
        <p key={w} className="warn">{w}</p>
      ))}

      <div className="result-grid">
        <MetricsTable strategy={result.metrics} benchmark={result.benchmark} market={result.market} />
        <EquityChart points={result.equity} market={result.market} />
      </div>

      <div className="panel">
        <h4>최신 목표 비중 (자산별)</h4>
        <div className="alloc-bars">
          {weights.map(([sym, pct]) => (
            <div key={sym} className="alloc-row">
              <span className="alloc-sym">
                <b>{sym}</b> <span className="muted small">{nameOf(sym) !== sym ? nameOf(sym) : ''}</span>{' '}
                <a className="ext-link" href={investingUrl(result.market, sym)} target="_blank" rel="noopener noreferrer">↗</a>
              </span>
              <div className="alloc-track"><i style={{ width: `${Math.min(100, Math.max(0, pct))}%` }} /></div>
              <span className="alloc-pct">{pct.toFixed(1)}%</span>
            </div>
          ))}
        </div>
      </div>

      <h4>리밸런싱 이력 ({result.rebalances.length}회)</h4>
      <div className="trades-wrap">
        <table className="trades left-first">
          <thead>
            <tr>
              <th>날짜</th>
              <th>사유</th>
              <th>목표 비중</th>
              <th>주문 수</th>
              <th>회전율</th>
            </tr>
          </thead>
          <tbody>
            {[...result.rebalances].reverse().slice(0, 60).map((r, idx) => (
              <tr key={`${r.date}-${idx}`}>
                <td>{r.date}</td>
                <td className="small">{r.reason}</td>
                <td className="small muted">
                  {Object.entries(r.weights)
                    .sort((a, b) => b[1] - a[1])
                    .map(([s, w]) => `${s} ${w.toFixed(0)}%`)
                    .join(' · ')}
                </td>
                <td>{r.trades}건</td>
                <td>{r.turnover_pct.toFixed(1)}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  )
}
