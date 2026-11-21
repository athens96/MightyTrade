import { useMemo } from 'react'
import type { Candle, Interval, Market } from '../../api/types'
import { diagnoseChart, type IndicatorSignal, type SignalTone } from '../../indicators/diagnosis'
import type { IndicatorConfig } from '../../indicators/registry'
import { priceFormatter } from './format'

interface Props {
  candles: Candle[]
  market: Market
  interval: Interval
  livePrice?: number | null
  activeIndicators?: IndicatorConfig[]
  defaultOpen?: boolean
}

const TONE_CLASS: Record<SignalTone, string> = {
  bull: 'sig-bull',
  bear: 'sig-bear',
  warn: 'sig-warn',
  cool: 'sig-cool',
  neutral: 'sig-neutral',
}

export default function ChartDiagnosis({
  candles,
  market,
  interval,
  livePrice,
  activeIndicators = [],
  defaultOpen = false,
}: Props) {
  const diag = useMemo(
    () => diagnoseChart(candles, market, interval, livePrice),
    [candles, market, interval, livePrice],
  )
  const money = priceFormatter(market)
  const activeIds = useMemo(() => new Set(activeIndicators.map((a) => a.id)), [activeIndicators])

  if (!diag) return null

  const trendSignals = diag.signals.filter((s) => s.category === 'trend')
  const timingSignals = diag.signals.filter((s) => s.category === 'timing')
  const volumeSignals = diag.signals.filter((s) => s.category === 'volume')

  const renderGroup = (title: string, items: IndicatorSignal[]) =>
    items.length > 0 && (
      <div className="diag-group">
        <h5>{title}</h5>
        <div className="diag-cards">
          {items.map((s) => (
            <div key={s.id} className={`diag-item ${TONE_CLASS[s.tone]}`}>
              <div className="diag-item-head">
                <b>{s.name}</b>
                {activeIds.has(s.id) && <span className="diag-active-tag">차트 표시중</span>}
                <span className="spacer" />
                <span className="diag-status">{s.status}</span>
              </div>
              <div className="diag-val muted small">{s.valueText}</div>
              <p className="small">{s.detail}</p>
            </div>
          ))}
        </div>
      </div>
    )

  return (
    <details className={`chart-diag tone-${diag.overallTone}`} open={defaultOpen}>
      <summary title="클릭하면 10여 개 보조지표를 종합한 현재 차트 추세·타이밍·거래량 해설을 펼치거나 접습니다">
        <span className="diag-badge">{diag.badge}</span>
        <span className="diag-score">종합 {diag.score}점</span>
        <span className="diag-headline">{diag.headline}</span>
        {diag.support && (
          <span className="diag-lv small muted">
            지지 <b>{money(diag.support.price)}</b>
          </span>
        )}
        {diag.resistance && (
          <span className="diag-lv small muted">
            저항 <b>{money(diag.resistance.price)}</b>
          </span>
        )}
        <span className="diag-more muted small">지표별 쉬운 해설</span>
      </summary>

      <div className="diag-body">
        <div className="diag-summary-box">
          <p>{diag.summary}</p>
          <p className="diag-tip">
            <b>💡 대응 힌트:</b> {diag.tip}
          </p>
        </div>

        <div className="diag-meters">
          <div className="diag-meter">
            <span className="muted small">추세 방향·힘</span>
            <b>{diag.trendScore}점</b>
          </div>
          <div className="diag-meter">
            <span className="muted small">현재 온도(타이밍)</span>
            <b>{diag.heatLabel}</b>
          </div>
          <div className="diag-meter">
            <span className="muted small">거래량·수급</span>
            <b>{diag.volumeScore}점</b>
          </div>
          <div className="diag-meter">
            <span className="muted small">평균 흔들림 (ATR)</span>
            <b>{diag.atr ? `${money(diag.atr.value)} (${diag.atr.pct.toFixed(1)}%)` : '-'}</b>
          </div>
          {diag.support && (
            <div className="diag-meter">
              <span className="muted small">가까운 지지선 ({diag.support.label})</span>
              <b className="down">{money(diag.support.price)}</b>
            </div>
          )}
          {diag.resistance && (
            <div className="diag-meter">
              <span className="muted small">가까운 저항선 ({diag.resistance.label})</span>
              <b className="up">{money(diag.resistance.price)}</b>
            </div>
          )}
        </div>

        {renderGroup('📈 추세 진단 (어느 방향으로 가고 있나요?)', trendSignals)}
        {renderGroup('🌡️ 타이밍 진단 (지금 비싼가요, 세일 구간인가요?)', timingSignals)}
        {renderGroup('📊 거래량·체결 진단 (진짜 돈이 실렸나요?)', volumeSignals)}

        <p className="muted small diag-disclaimer">
          위 해설은 차트의 10여 개 기술적 지표를 공식에 따라 자동 요약한 참고 정보이며, 기업 실적·뉴스는 반영하지 않습니다.
        </p>
      </div>
    </details>
  )
}
