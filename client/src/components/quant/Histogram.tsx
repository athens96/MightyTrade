import { useState } from 'react'
import { pct, scale, ticks } from './svg'

const W = 560
const H = 220
const M = { l: 12, r: 12, t: 22, b: 30 }
const BAR = '#4fc3f7'

interface Props {
  title: string
  edges: number[]
  counts: number[]
  actual: number // marker for the real backtest value
  median: number
}

/** Distribution of simulated outcomes (one series), with the actual result and the median marked. */
export default function Histogram({ title, edges, counts, actual, median }: Props) {
  const [hover, setHover] = useState<number | null>(null)
  const total = counts.reduce((a, b) => a + b, 0)
  const lo = Math.min(edges[0], actual)
  const hi = Math.max(edges[edges.length - 1], actual)
  const x = scale(lo, hi, M.l, W - M.r)
  const y = scale(0, Math.max(...counts), H - M.b, M.t)
  const marker = (v: number, label: string, cls: string, anchor: 'start' | 'end') => (
    <g>
      <line x1={x(v)} x2={x(v)} y1={M.t - 4} y2={H - M.b} className={cls} />
      <text x={x(v) + (anchor === 'start' ? 4 : -4)} y={M.t - 8} className="tick" textAnchor={anchor}>
        {label} {pct(v, 1)}
      </text>
    </g>
  )

  return (
    <div className="svg-chart">
      <div className="small muted">{title}</div>
      <svg viewBox={`0 0 ${W} ${H}`} onMouseLeave={() => setHover(null)} role="img" aria-label={title}>
        {counts.map((c, i) => {
          const x0 = x(edges[i]) + 1 // 2px surface gap between bars
          const w = Math.max(1, x(edges[i + 1]) - x(edges[i]) - 2)
          return (
            <g key={i} onMouseEnter={() => setHover(i)}>
              <rect x={x(edges[i])} y={M.t} width={x(edges[i + 1]) - x(edges[i])} height={H - M.b - M.t} fill="transparent" />
              <rect x={x0} y={y(c)} width={w} height={H - M.b - y(c)} rx={2} fill={BAR} opacity={hover === null || hover === i ? 1 : 0.5} />
            </g>
          )
        })}
        <line x1={M.l} x2={W - M.r} y1={H - M.b} y2={H - M.b} className="grid" />
        {ticks(lo, hi, 4).map((t) => (
          <text key={t} x={x(t)} y={H - M.b + 16} className="tick" textAnchor="middle">{pct(t)}</text>
        ))}
        {marker(median, '중앙', 'marker-median', actual > median ? 'end' : 'start')}
        {marker(actual, '실제', 'marker-actual', actual > median ? 'start' : 'end')}
      </svg>
      {hover !== null && (
        <div className="svg-tooltip" style={{ left: `${(x((edges[hover] + edges[hover + 1]) / 2) / W) * 100}%` }}>
          <div className="tt-title">
            {pct(edges[hover], 1)} ~ {pct(edges[hover + 1], 1)}
          </div>
          <div className="tt-row">
            <span>시뮬레이션</span>
            <b>
              {counts[hover]}회 ({((counts[hover] / total) * 100).toFixed(1)}%)
            </b>
          </div>
        </div>
      )}
    </div>
  )
}
