import { useNavigate } from 'react-router-dom'
import { PREFILL_KEY } from '../../api/autotrade'
import type { MonteCarloResult } from '../../api/quant'
import { day } from './BacktestView'
import FanChart from './FanChart'
import { fmtMetric } from './Heatmap'
import Histogram from './Histogram'

const P = ['p5', 'p25', 'p50', 'p75', 'p95'] as const

export default function MonteCarloView({ result, title }: { result: MonteCarloResult; title: string }) {
  const navigate = useNavigate()
  const o = result.original
  const makeBot = () => {
    sessionStorage.setItem(PREFILL_KEY, JSON.stringify({ market: result.market, symbol: result.symbol, strategy: result.strategy, params: result.params }))
    navigate('/auto')
  }
  return (
    <>
      <div className="result-head">
        <div className="account-head">
          <h3>{title} · 몬테카를로</h3>
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
          {day(result.period.start)} ~ {day(result.period.end)} · 청산 매매 {result.trades}회 ·{' '}
          {result.method === 'bootstrap' ? '부트스트랩(복원 추출)' : '순서 섞기'} {result.runs.toLocaleString()}회
        </span>
      </div>
      {result.warnings.map((w) => <p key={w} className="warn">{w}</p>)}
      <div className="stat-tiles">
        <div className="stat-tile">
          <span className="muted small">손실로 끝날 확률</span>
          <b>{result.prob_loss_pct.toFixed(1)}%</b>
        </div>
        <div className="stat-tile">
          <span className="muted small">연수익률 중앙값 (5%~95%)</span>
          <b>{fmtMetric('cagr_pct', result.cagr_pct.p50)}</b>
          <span className="small muted">{fmtMetric('cagr_pct', result.cagr_pct.p5)} ~ {fmtMetric('cagr_pct', result.cagr_pct.p95)}</span>
        </div>
        <div className="stat-tile">
          <span className="muted small">최대 낙폭: 나쁜 경우 5%</span>
          <b>{fmtMetric('mdd_pct', result.mdd_pct.p5)}</b>
          <span className="small muted">실제 백테스트 {fmtMetric('mdd_pct', o.mdd_pct)}</span>
        </div>
        <div className="stat-tile">
          <span className="muted small">실제보다 낙폭이 컸던 비율</span>
          <b>{result.prob_mdd_worse_pct.toFixed(0)}%</b>
          <span className="small muted">높을수록 백테스트 낙폭은 운이 좋았던 것</span>
        </div>
      </div>
      <FanChart index={result.path_index} bands={result.bands} original={o.path} />
      <div className="hist-row">
        <Histogram title="최종 수익률 분포" edges={result.hist_final.edges} counts={result.hist_final.counts}
                   actual={o.final_return_pct} median={result.final_return_pct.p50} />
        <Histogram title="최대 낙폭(MDD) 분포" edges={result.hist_mdd.edges} counts={result.hist_mdd.counts}
                   actual={o.mdd_pct} median={result.mdd_pct.p50} />
      </div>
      <table className="metrics mc-table">
        <thead>
          <tr>
            <th />
            {P.map((p) => <th key={p}>{p === 'p50' ? '중앙값' : `${p.slice(1)}%`}</th>)}
            <th>실제</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>최종 수익률</td>
            {P.map((p) => <td key={p}>{fmtMetric('total_return_pct', result.final_return_pct[p])}</td>)}
            <td>{fmtMetric('total_return_pct', o.final_return_pct)}</td>
          </tr>
          <tr>
            <td>연수익률</td>
            {P.map((p) => <td key={p}>{fmtMetric('cagr_pct', result.cagr_pct[p])}</td>)}
            <td>{fmtMetric('cagr_pct', result.backtest_metrics.cagr_pct)}</td>
          </tr>
          <tr>
            <td>최대 낙폭</td>
            {P.map((p) => <td key={p}>{fmtMetric('mdd_pct', result.mdd_pct[p])}</td>)}
            <td>{fmtMetric('mdd_pct', o.mdd_pct)}</td>
          </tr>
        </tbody>
      </table>
      <p className="muted small explain">
        각 매매가 계좌에 준 영향(손익 ÷ 진입 직전 자산)을 {result.method === 'bootstrap' ? '무작위로 다시 뽑아' : '순서만 섞어'} 수천 개의
        가상 결과를 만들었습니다. 매매 단위 계산이라 현금 보유 기간과 매매 간 상관관계는 반영하지 않습니다. 실제 투자 시 감당할 손실은 실제
        백테스트의 MDD가 아니라 <b>나쁜 경우 5%</b> 값을 기준으로 잡는 것이 안전합니다.
      </p>
    </>
  )
}
