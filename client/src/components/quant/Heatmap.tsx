import { useRef, useState } from 'react'
import type { Axis, CellMetrics, GridCell, GridPick } from '../../api/quant'

// Diverging scale: blue (worse than baseline) ↔ gray midpoint ↔ red (better), matching KR up/down colors.
// Validated against the #111418 surface with the dataviz palette checker.
const NEG = [0x3e, 0x63, 0xdd]
const MID = [0x38, 0x38, 0x35]
const POS = [0xe5, 0x48, 0x4d]

const mix = (pole: number[], a: number) => {
  const c = MID.map((m, i) => Math.round(m + (pole[i] - m) * Math.max(0, Math.min(1, a))))
  return `rgb(${c[0]},${c[1]},${c[2]})`
}

/**
 * Values on both sides of the baseline -> diverging (blue ↔ gray ↔ red, symmetric around the baseline).
 * All on one side -> sequential in that side's hue, stretched over the actual min..max so differences show.
 */
function colorScale(scores: number[], baseline: number) {
  const lo = Math.min(...scores)
  const hi = Math.max(...scores)
  if (lo < baseline && hi > baseline) {
    const m = Math.max(baseline - lo, hi - baseline)
    return { color: (v: number) => mix(v < baseline ? NEG : POS, Math.abs(v - baseline) / m), lo: baseline - m, hi: baseline + m,
             ramp: 'linear-gradient(90deg, #3e63dd, #383835, #e5484d)' }
  }
  const up = lo >= baseline
  const span = hi - lo || 1
  // keep a little color at the weak end so cells never vanish into the surface
  const t = (v: number) => 0.2 + 0.8 * (up ? (v - lo) / span : (hi - v) / span)
  return { color: (v: number) => mix(up ? POS : NEG, t(v)), lo, hi,
           ramp: up ? 'linear-gradient(90deg, rgb(78,64,64), #e5484d)' : 'linear-gradient(90deg, #3e63dd, rgb(56,64,90))' }
}

export const fmtMetric = (key: string, v: number | null | undefined) => {
  if (v === null || v === undefined) return '-'
  if (key.endsWith('_pct')) return `${v > 0 && key !== 'mdd_pct' && key !== 'win_rate_pct' ? '+' : ''}${v.toFixed(1)}%`
  if (key === 'trades') return String(v)
  return v.toFixed(2)
}

const TOOLTIP_ROWS: [keyof CellMetrics, string][] = [
  ['cagr_pct', 'CAGR'],
  ['mdd_pct', 'MDD'],
  ['sharpe', '샤프'],
  ['calmar', '칼마'],
  ['profit_factor', '손익비'],
  ['win_rate_pct', '승률'],
  ['trades', '매매'],
]

interface Props {
  x: Axis
  y: Axis | null
  cells: (GridCell | null)[][]
  objective: string
  objectiveLabel: string
  best: GridPick | null
  robust: GridPick | null
  onPick: (params: Record<string, number>) => void
}

export default function Heatmap({ x, y, cells, objective, objectiveLabel, best, robust, onPick }: Props) {
  const [hover, setHover] = useState<{ xi: number; yi: number; left: number; top: number } | null>(null)
  const wrap = useRef<HTMLDivElement>(null)
  const baseline = objective === 'profit_factor' ? 1 : 0
  const scores = cells.flat().flatMap((c) => (c && 'score' in c && c.score !== null ? [c.score] : []))
  const scale = colorScale(scores.length ? scores : [baseline], baseline)
  // tooltip sits next to the hovered cell (flips to the left near the right edge)
  const hoverAt = (xi: number, yi: number, el: HTMLElement) => {
    const box = wrap.current!.getBoundingClientRect()
    const r = el.getBoundingClientRect()
    const right = r.right - box.left + 8
    const left = right + 190 > box.width ? r.left - box.left - 198 : right
    setHover({ xi, yi, left, top: r.top - box.top })
  }
  const rows = y ? [...y.values.keys()].reverse() : [0] // largest Y at the top
  const showValues = x.values.length <= 12 && (y?.values.length ?? 1) <= 14
  const hovered = hover ? cells[hover.yi]?.[hover.xi] : null

  return (
    <div className="heatmap" ref={wrap}>
      <div className="heatmap-legend">
        <span className="muted small">{objectiveLabel}</span>
        <span className="small">{fmtMetric(objective, scale.lo)}</span>
        <span className="heatmap-ramp" style={{ background: scale.ramp }} />
        <span className="small">{fmtMetric(objective, scale.hi)}</span>
        <span className="muted small">★ 최고값 · ◆ 가장 안정적(주변 평균 최고) · 빗금 = 매매 수 부족 · 클릭하면 그 설정으로 백테스트</span>
      </div>
      {y && <div className="heatmap-ylabel">↑ {y.label}</div>}
      <div className="heatmap-body">
        <div
          className="heatmap-grid"
          style={{ gridTemplateColumns: `auto repeat(${x.values.length}, minmax(26px, 1fr))` }}
          onMouseLeave={() => setHover(null)}
        >
          {rows.map((yi) => (
            <div className="heatmap-row" key={yi} style={{ display: 'contents' }}>
              <div className="heatmap-tick">{y ? y.values[yi] : ''}</div>
              {x.values.map((_, xi) => {
                const c = cells[yi]?.[xi]
                const isBest = best && best.x === xi && best.y === yi
                const isRobust = robust && robust.x === xi && robust.y === yi
                if (!c || 'invalid' in c) {
                  return <div key={xi} className="heatmap-cell invalid" title={c && 'invalid' in c ? c.invalid : ''} />
                }
                const bg = c.score === null ? '#22262c' : scale.color(c.score)
                return (
                  <button
                    key={xi}
                    className={`heatmap-cell${c.eligible ? '' : ' ineligible'}${hover?.xi === xi && hover?.yi === yi ? ' hover' : ''}`}
                    style={{ background: bg }}
                    onMouseEnter={(e) => hoverAt(xi, yi, e.currentTarget)}
                    onFocus={(e) => hoverAt(xi, yi, e.currentTarget)}
                    onClick={() => onPick(c.params)}
                    aria-label={`${x.label} ${x.values[xi]}${y ? `, ${y.label} ${y.values[yi]}` : ''}: ${fmtMetric(objective, c.score)}`}
                  >
                    {isBest && <span className="mark">★</span>}
                    {isRobust && <span className="mark">◆</span>}
                    {showValues && <span className="val">{fmtMetric(objective, c.score)}</span>}
                  </button>
                )
              })}
            </div>
          ))}
          <div />
          {x.values.map((v) => (
            <div key={v} className="heatmap-tick x">
              {v}
            </div>
          ))}
        </div>
      </div>
      <div className="heatmap-xlabel">{x.label}</div>
      {hover && hovered && 'params' in hovered && (
        <div className="heatmap-tooltip" style={{ left: hover.left, top: hover.top }}>
          <div className="tt-title">
            {x.label} {x.values[hover.xi]}
            {y && ` · ${y.label} ${y.values[hover.yi]}`}
          </div>
          <div className="tt-row">
            <span>{objectiveLabel}</span>
            <b>{fmtMetric(objective, hovered.score)}</b>
          </div>
          <div className="tt-row">
            <span>주변 평균 ({hovered.neighbors}칸)</span>
            <b>{fmtMetric(objective, hovered.neighbor_avg)}</b>
          </div>
          {TOOLTIP_ROWS.filter(([k]) => k !== objective).map(([k, label]) => (
            <div className="tt-row" key={k}>
              <span>{label}</span>
              <b>{fmtMetric(k, hovered.metrics[k])}</b>
            </div>
          ))}
          {!hovered.eligible && <div className="muted small">매매 수가 적어 최고값 후보에서 제외</div>}
        </div>
      )}
    </div>
  )
}
