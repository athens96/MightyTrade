import type { GridPick, OptimizeResult } from '../../api/quant'
import { day } from './BacktestView'
import Heatmap, { fmtMetric } from './Heatmap'

function PickCard({ title, hint, pick, objective, objectiveLabel, onPick }: {
  title: string; hint: string; pick: GridPick | null; objective: string; objectiveLabel: string
  onPick: (p: Record<string, number>) => void
}) {
  if (!pick) return <div className="pick-card muted">{title}: 조건을 만족하는 조합 없음</div>
  return (
    <div className="pick-card">
      <div className="pick-title">{title} <span className="muted small">{hint}</span></div>
      <div className="pick-params">{Object.entries(pick.params).map(([k, v]) => `${k}=${v}`).join(', ')}</div>
      <div className="pick-stats small">
        <span>{objectiveLabel} <b>{fmtMetric(objective, pick.score)}</b></span>
        <span>주변 평균 <b>{fmtMetric(objective, pick.neighbor_avg)}</b></span>
        <span>CAGR <b>{fmtMetric('cagr_pct', pick.metrics.cagr_pct)}</b></span>
        <span>MDD <b>{fmtMetric('mdd_pct', pick.metrics.mdd_pct)}</b></span>
        <span>매매 <b>{pick.metrics.trades}</b></span>
      </div>
      <button
        className="link"
        title="이 파라미터 숫자들을 적용해 전체 기간 상세 백테스트와 매매 타점 차트를 실행합니다"
        onClick={() => onPick(pick.params)}
      >
        이 설정으로 백테스트 →
      </button>
    </div>
  )
}

function onEdge(r: OptimizeResult) {
  const b = r.best!
  const xEdge = r.x.values.length > 2 && (b.x === 0 || b.x === r.x.values.length - 1)
  const yEdge = !!r.y && r.y.values.length > 2 && (b.y === 0 || b.y === r.y.values.length - 1)
  return xEdge || yEdge
}

export default function OptimizeView({ result, title, objectiveLabel, onPick }: {
  result: OptimizeResult; title: string; objectiveLabel: string; onPick: (p: Record<string, number>) => void
}) {
  const total = result.cells.flat().length
  return (
    <>
      <div className="result-head">
        <h3>{title} · 파라미터 최적화</h3>
        <span className="muted">
          {day(result.period.start)} ~ {day(result.period.end)} · {total}개 조합 · {result.elapsed_sec}초 · 최소 매매 {result.min_trades}회 ·
          단순 보유 CAGR {fmtMetric('cagr_pct', result.benchmark.cagr_pct)} / {objectiveLabel}{' '}
          {fmtMetric(result.objective, (result.benchmark as unknown as Record<string, number | null>)[result.objective])}
        </span>
      </div>
      {result.warnings.map((w) => <p key={w} className="warn">{w}</p>)}
      {result.best && onEdge(result) && (
        <p className="warn">최고값이 탐색 범위의 끝에 있습니다. 범위 밖에 더 좋은 값이 있을 수 있으니 범위를 넓혀 다시 확인하세요.</p>
      )}
      <div className="pick-cards">
        <PickCard title="★ 최고값" hint="과거에 가장 좋았던 조합" pick={result.best} objective={result.objective} objectiveLabel={objectiveLabel} onPick={onPick} />
        <PickCard title="◆ 가장 안정적" hint="주변 조합까지 고르게 좋은 곳 (권장)" pick={result.robust} objective={result.objective} objectiveLabel={objectiveLabel} onPick={onPick} />
      </div>
      <Heatmap x={result.x} y={result.y} cells={result.cells} objective={result.objective} objectiveLabel={objectiveLabel}
               best={result.best} robust={result.robust} onPick={onPick} />
      <p className="muted small explain">
        읽는 법: 최고값 한 칸만 튀고 주변이 나쁘면 과거 데이터에 우연히 맞춘 <b>과최적화</b>일 가능성이 큽니다. 넓은 영역이 고르게 좋은 곳(◆)이
        미래에도 통할 가능성이 높습니다. 고른 설정은 <b>워크포워드</b> 탭에서 처음 보는 기간에도 통하는지 확인하세요.
      </p>
    </>
  )
}
