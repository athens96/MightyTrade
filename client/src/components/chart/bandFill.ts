// Series primitive that fills the area between two lines (Bollinger band, Ichimoku cloud).
// lightweight-charts has no native "fill between", so we draw it on the pane canvas.
import type {
  IChartApiBase,
  IPrimitivePaneRenderer,
  IPrimitivePaneView,
  ISeriesApi,
  ISeriesPrimitive,
  SeriesAttachedParameter,
  SeriesType,
  Time,
} from 'lightweight-charts'

export interface BandPoint {
  time: Time
  upper: number
  lower: number
}

type Target = Parameters<IPrimitivePaneRenderer['draw']>[0]

export class BandFill implements ISeriesPrimitive<Time> {
  private chart: IChartApiBase<Time> | null = null
  private series: ISeriesApi<SeriesType, Time> | null = null
  private requestUpdate: (() => void) | null = null
  private points: BandPoint[] = []
  private readonly view: IPrimitivePaneView
  private readonly fillUp: string
  private readonly fillDown: string

  constructor(fillUp: string, fillDown: string) {
    this.fillUp = fillUp
    this.fillDown = fillDown
    const renderer: IPrimitivePaneRenderer = { draw: (target) => this.draw(target) }
    this.view = { zOrder: () => 'bottom', renderer: () => renderer }
  }

  attached(p: SeriesAttachedParameter<Time>) {
    this.chart = p.chart
    this.series = p.series
    this.requestUpdate = p.requestUpdate
  }

  detached() {
    this.chart = null
    this.series = null
    this.requestUpdate = null
  }

  setPoints(points: BandPoint[]) {
    this.points = points
    this.requestUpdate?.()
  }

  paneViews() {
    return [this.view]
  }

  private draw(target: Target) {
    const { chart, series } = this
    if (!chart || !series || this.points.length < 2) return
    const ts = chart.timeScale()
    const pts = this.points.map((p) => ({
      x: ts.timeToCoordinate(p.time),
      u: series.priceToCoordinate(p.upper),
      l: series.priceToCoordinate(p.lower),
      up: p.upper >= p.lower,
    }))
    target.useBitmapCoordinateSpace(({ context: ctx, horizontalPixelRatio: hr, verticalPixelRatio: vr }) => {
      for (let i = 1; i < pts.length; i++) {
        const a = pts[i - 1]
        const b = pts[i]
        if (a.x === null || b.x === null || a.u === null || a.l === null || b.u === null || b.l === null) continue
        ctx.fillStyle = a.up ? this.fillUp : this.fillDown
        ctx.beginPath()
        ctx.moveTo(a.x * hr, a.u * vr)
        ctx.lineTo(b.x * hr, b.u * vr)
        ctx.lineTo(b.x * hr, b.l * vr)
        ctx.lineTo(a.x * hr, a.l * vr)
        ctx.closePath()
        ctx.fill()
      }
    })
  }
}
