import type { WalkForwardResult } from '../../api/quant'
import { day } from './BacktestView'
import EquityChart from './EquityChart'
import { fmtMetric } from './Heatmap'
import MetricsTable from './MetricsTable'

function efficiencyNote(e: number | null) {
  if (e === null) return '학습 구간 수익이 없어 계산할 수 없습니다'
  if (e > 1.5) return '검증 성과가 학습보다 훨씬 높음 — 특정 구간(급등장 등) 영향일 수 있으니 구간별 결과를 확인하세요'
  if (e >= 0.5) return '학습 구간 성과의 절반 이상이 처음 보는 구간에서도 유지됨 — 비교적 견고'
  if (e > 0) return '처음 보는 구간에서 성과가 크게 줄어듦 — 과최적화 가능성'
  return '처음 보는 구간에서는 손실 — 이 전략·범위는 신뢰하기 어려움'
}

export default function WalkForwardView({ result, title, objectiveLabel }: { result: WalkForwardResult; title: string; objectiveLabel: string }) {
  const e = result.efficiency
  return (
    <>
      <div className="result-head">
        <h3>{title} · 워크포워드 검증</h3>
        <span className="muted">
          {result.mode === 'rolling' ? '롤링' : '누적'} · 학습 {result.is_years}년 → 검증 {result.oos_months}개월 × {result.windows.length}구간 ·
          목표 {objectiveLabel} · {result.elapsed_sec}초
        </span>
      </div>
      {result.warnings.map((w) => <p key={w} className="warn">{w}</p>)}
      <div className="stat-tiles">
        <div className="stat-tile">
          <span className="muted small">검증 구간만 이어붙인 CAGR</span>
          <b>{fmtMetric('cagr_pct', result.metrics.cagr_pct)}</b>
        </div>
        <div className="stat-tile">
          <span className="muted small">같은 기간 단순 보유</span>
          <b>{fmtMetric('cagr_pct', result.benchmark.cagr_pct)}</b>
        </div>
        <div className="stat-tile">
          <span className="muted small">학습 구간 평균 CAGR</span>
          <b>{fmtMetric('cagr_pct', result.mean_is_cagr_pct)}</b>
        </div>
        <div className="stat-tile">
          <span className="muted small">워크포워드 효율 (검증÷학습)</span>
          <b>{e === null ? '-' : `${(e * 100).toFixed(0)}%`}</b>
          <span className="small">{efficiencyNote(e)}</span>
        </div>
      </div>
      <div className="result-grid">
        <MetricsTable strategy={result.metrics} benchmark={result.benchmark} market={result.market} />
        <EquityChart points={result.equity} market={result.market} />
      </div>
      <h4>구간별 결과</h4>
      <div className="trades-wrap">
        <table className="trades">
          <thead>
            <tr>
              <th>#</th>
              <th>학습 기간</th>
              <th>선택된 설정</th>
              <th>학습 {objectiveLabel}</th>
              <th>학습 CAGR</th>
              <th>검증 기간</th>
              <th>검증 수익률</th>
              <th>검증 MDD</th>
              <th>검증 매매</th>
            </tr>
          </thead>
          <tbody>
            {result.windows.map((w, i) => (
              <tr key={w.oos_start}>
                <td className="muted">{i + 1}</td>
                <td>{day(w.is_start)} ~ {day(w.is_end)}</td>
                <td>{w.params ? Object.entries(w.params).map(([k, v]) => `${k}=${v}`).join(', ') : <span className="muted">후보 없음 (현금 보유)</span>}</td>
                <td>{fmtMetric(result.objective, w.is_score)}</td>
                <td className={(w.is_cagr_pct ?? 0) > 0 ? 'up' : 'down'}>{fmtMetric('cagr_pct', w.is_cagr_pct)}</td>
                <td>{day(w.oos_start)} ~ {day(w.oos_end)}</td>
                <td className={(w.oos?.total_return_pct ?? 0) > 0 ? 'up' : 'down'}>{fmtMetric('total_return_pct', w.oos?.total_return_pct)}</td>
                <td>{fmtMetric('mdd_pct', w.oos?.mdd_pct)}</td>
                <td>{w.oos?.trades ?? '-'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="muted small explain">
        각 구간마다 학습 기간에서만 최적 설정을 고르고, 그 설정으로 바로 다음 검증 기간을 거래했습니다(검증 기간의 데이터는 설정 선택에 쓰이지 않음).
        구간마다 선택된 설정이 크게 흔들리면 전략이 특정 시기에만 맞았을 수 있습니다.
      </p>
    </>
  )
}
