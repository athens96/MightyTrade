import type { StrategyGuide } from '../api/quant'
import type { FeatureGuide } from '../lib/featureGuides'

export function StrategyGuideBox({
  title = '이론적 배경 · 세팅 & 실행 가이드',
  theory,
  guide,
  defaultOpen = false,
}: {
  title?: string
  theory?: string
  guide?: StrategyGuide
  defaultOpen?: boolean
}) {
  if (!guide || (!guide.background && !guide.setup?.length && !guide.execution?.length)) return null
  return (
    <details className="guide-box" open={defaultOpen}>
      <summary>
        <span>📘 {title}</span>
        {theory && <span className="muted small"> · {theory}</span>}
      </summary>
      <div className="guide-body small">
        {guide.background && (
          <section>
            <h5>이론적 배경</h5>
            <p>{guide.background}</p>
          </section>
        )}
        {guide.setup && guide.setup.length > 0 && (
          <section>
            <h5>추천 세팅 방법</h5>
            <ul>
              {guide.setup.map((line, i) => <li key={i}>{line}</li>)}
            </ul>
          </section>
        )}
        {guide.execution && guide.execution.length > 0 && (
          <section>
            <h5>실제 진행·실행 흐름</h5>
            <ol>
              {guide.execution.map((line, i) => <li key={i}>{line}</li>)}
            </ol>
          </section>
        )}
        {guide.caution && (
          <p className="guide-caution"><b>⚠ 주의사항:</b> {guide.caution}</p>
        )}
      </div>
    </details>
  )
}

export function FeatureGuideBox({ guide }: { guide: FeatureGuide }) {
  return (
    <details className="guide-box feature-guide">
      <summary>
        <span>🧭 {guide.title}</span>
        <span className="muted small"> · 세팅 방법 및 진행 흐름 보기</span>
      </summary>
      <div className="guide-body small">
        <p className="muted">{guide.overview}</p>
        <div className="guide-cols">
          <section>
            <h5>세팅 방법</h5>
            <ul>
              {guide.setup.map((s, i) => <li key={i}>{s}</li>)}
            </ul>
          </section>
          <section>
            <h5>진행 흐름</h5>
            <ol>
              {guide.flow.map((f, i) => <li key={i}>{f}</li>)}
            </ol>
          </section>
        </div>
        {guide.tips && guide.tips.length > 0 && (
          <section>
            <h5>실전 팁 · 안전장치</h5>
            <ul>
              {guide.tips.map((t, i) => <li key={i}>{t}</li>)}
            </ul>
          </section>
        )}
      </div>
    </details>
  )
}
