import { useState } from 'react'
import type { RiskAudit } from '../../api/quant'

const ICON: Record<'pass' | 'warn' | 'fail', string> = {
  pass: '✅',
  warn: '⚠️',
  fail: '❌',
}

export default function RiskAuditCard({ audit }: { audit?: RiskAudit }) {
  const [open, setOpen] = useState(true)
  if (!audit || !audit.gates?.length) return null

  const tone = audit.verdict.toLowerCase() // 'pass' | 'caution' | 'fail'

  return (
    <div className={`panel risk-audit risk-audit-${tone}`}>
      <div className="risk-audit-head">
        <div className="risk-audit-title">
          <span className={`risk-audit-badge ${tone}`}>{audit.verdict_label}</span>
          <strong title="《151 Trading Strategies》 리스크 매니저 검열 원칙: 전략 수익이 우연·소수 거래 편중·수수료 착시가 아닌지 6개 관점으로 자동 점검합니다.">
            6-Gate 리스크 매니저 자동 검열
          </strong>
          <span className="muted small">
            통과 {audit.passed} · 주의 {audit.warned} · 탈락 {audit.failed}
          </span>
        </div>
        <button type="button" className="ghost small" onClick={() => setOpen((v) => !v)}>
          {open ? '접기 ▲' : '상세 보기 ▼'}
        </button>
      </div>
      <p className="risk-audit-summary">{audit.summary}</p>
      {open && (
        <div className="risk-audit-grid">
          {audit.gates.map((g) => (
            <div key={g.id} className={`risk-gate risk-gate-${g.status}`}>
              <div className="risk-gate-top">
                <span className="risk-gate-name">
                  {ICON[g.status]} {g.title}
                </span>
                <strong className="risk-gate-val">{g.value}</strong>
              </div>
              <p className="risk-gate-detail">{g.detail}</p>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
