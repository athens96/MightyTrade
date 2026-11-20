import { useRef, useState } from 'react'
import { pct, scale, ticks } from './svg'

const BAND = '79,195,247' // single hue, opacity encodes the percentile band
const W = 1100
const H = 360
const M = { l: 56, r: 16, t: 12, b: 32 }

interface Props {
  index: number[] // trade number for each point
  bands: { p5: number[]; p25: number[]; p50: number[]; p75: number[]; p95: number[] }
  original: number[]
}

/** Percentile fan of simulated equity paths (cumulative return %) by trade number, with the actual path. */
export default function FanChart({ index, bands, original }: Props) {
  const ref = useRef<SVGSVGElement>(null)
  const [hover, setHover] = useState<number | null>(null)
  const lo = Math.min(0, ...bands.p5, ...original)
  const hi = Math.max(0, ...bands.p95, ...original)
  const x = scale(index[0], index[index.length - 1], M.l, W - M.r)
  const y = scale(lo, hi, H - M.b, M.t)
  const line = (vals: number[]) => vals.map((v, i) => `${i ? 'L' : 'M'}${x(index[i]).toFixed(1)},${y(v).toFixed(1)}`).join('')
  const area = (top: number[], bottom: number[]) =>
    `${line(top)}${[...bottom.keys()].reverse().map((i) => `L${x(index[i]).toFixed(1)},${y(bottom[i]).toFixed(1)}`).join('')}Z`

  const onMove = (e: React.MouseEvent) => {
    const r = ref.current!.getBoundingClientRect()
    const px = ((e.clientX - r.left) / r.width) * W
    let best = 0
    index.forEach((t, i) => Math.abs(x(t) - px) < Math.abs(x(index[best]) - px) && (best = i))
    setHover(best)
  }
  const h = hover

  return (
    <div className="svg-chart">
      <div className="svg-legend small">
        <span><i className="sw" style={{ background: `rgba(${BAND},0.18)` }} />5–95%</span>
        <span><i className="sw" style={{ background: `rgba(${BAND},0.4)` }} />25–75%</span>
        <span><i className="sw line" style={{ background: `rgb(${BAND})` }} />중앙값</span>
        <span><i className="sw line dashed" />실제 순서</span>
      </div>
      <svg ref={ref} viewBox={`0 0 ${W} ${H}`} onMouseMove={onMove} onMouseLeave={() => setHover(null)} role="img"
           aria-label="몬테카를로 누적 수익률 분포">
        {ticks(lo, hi).map((t) => (
          <g key={t}>
            <line x1={M.l} x2={W - M.r} y1={y(t)} y2={y(t)} className={t === 0 ? 'axis-zero' : 'grid'} />
            <text x={M.l - 6} y={y(t) + 4} className="tick" textAnchor="end">{pct(t)}</text>
          </g>
        ))}
        {ticks(index[0], index[index.length - 1]).map((t) => (
          <text key={t} x={x(t)} y={H - M.b + 18} className="tick" textAnchor="middle">{Math.round(t)}</text>
        ))}
        <text x={W - M.r} y={H - 4} className="tick" textAnchor="end">매매 횟수 →</text>
        <path d={area(bands.p95, bands.p5)} fill={`rgba(${BAND},0.18)`} />
        <path d={area(bands.p75, bands.p25)} fill={`rgba(${BAND},0.4)`} />
        <path d={line(bands.p50)} fill="none" stroke={`rgb(${BAND})`} strokeWidth={2} />
        <path d={line(original)} fill="none" stroke="#e6edf3" strokeWidth={1.5} strokeDasharray="5 4" />
        {h !== null && <line x1={x(index[h])} x2={x(index[h])} y1={M.t} y2={H - M.b} className="crosshair" />}
      </svg>
      {h !== null && (
        <div className="svg-tooltip" style={{ left: `${(x(index[h]) / W) * 100}%` }}>
          <div className="tt-title">{index[h]}번째 매매 후</div>
          {(['p95', 'p75', 'p50', 'p25', 'p5'] as const).map((k) => (
            <div className="tt-row" key={k}>
              <span>{k === 'p50' ? '중앙값' : `${k.slice(1)}%`}</span>
              <b>{pct(bands[k][h], 1)}</b>
            </div>
          ))}
          <div className="tt-row">
            <span>실제</span>
            <b>{pct(original[h], 1)}</b>
          </div>
        </div>
      )}
    </div>
  )
}
