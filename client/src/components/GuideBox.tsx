import type { StrategyGuide } from '../api/quant'
import type { FeatureGuide } from '../lib/featureGuides'

export function StrategyGuideBox({
  title = '쉽게 읽는 이론 · 세팅 & 진행 가이드',
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
      <summary title="클릭하면 이 투자 전략의 원리·권장 세팅·주의사항 가이드를 펼치거나 접습니다">
        <span>📘 {title}</span>
        {theory && <span className="muted small"> · {theory}</span>}
      </summary>
      <div className="guide-body small">
        {guide.background && (
          <section>
            <h5>💡 왜 이런 원리로 투자하나요?</h5>
            <p>{guide.background}</p>
          </section>
        )}
        {guide.setup && guide.setup.length > 0 && (
          <section>
            <h5>🛠️ 이렇게 세팅해 보세요</h5>
            <ul>
              {guide.setup.map((line, i) => <li key={i}>{line}</li>)}
            </ul>
          </section>
        )}
        {guide.execution && guide.execution.length > 0 && (
          <section>
            <h5>⚙️ 실제로는 이렇게 진행돼요</h5>
            <ol>
              {guide.execution.map((line, i) => <li key={i}>{line}</li>)}
            </ol>
          </section>
        )}
        {guide.caution && (
          <p className="guide-caution"><b>⚠️ 이럴 때는 조심하세요:</b> {guide.caution}</p>
        )}
      </div>
    </details>
  )
}

export function FeatureGuideBox({ guide }: { guide: FeatureGuide }) {
  return (
    <details className="guide-box feature-guide">
      <summary title="클릭하면 이 화면의 사용 순서와 안전장치 안내를 펼치거나 접습니다">
        <span>🧭 {guide.title}</span>
        <span className="muted small"> · 처음 쓰시나요? 세팅·진행 방법 보기</span>
      </summary>
      <div className="guide-body small">
        <p className="muted">{guide.overview}</p>
        <div className="guide-cols">
          <section>
            <h5>🛠️ 이렇게 세팅하세요</h5>
            <ul>
              {guide.setup.map((s, i) => <li key={i}>{s}</li>)}
            </ul>
          </section>
          <section>
            <h5>⚙️ 이렇게 진행돼요</h5>
            <ol>
              {guide.flow.map((f, i) => <li key={i}>{f}</li>)}
            </ol>
          </section>
        </div>
        {guide.tips && guide.tips.length > 0 && (
          <section>
            <h5>💡 알아두면 좋은 팁 · 안전장치</h5>
            <ul>
              {guide.tips.map((t, i) => <li key={i}>{t}</li>)}
            </ul>
          </section>
        )}
      </div>
    </details>
  )
}
