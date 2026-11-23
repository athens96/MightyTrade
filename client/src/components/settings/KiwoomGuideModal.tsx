import { useEffect, useState } from 'react'
import {
  KIWOOM_FAQ_ITEMS,
  KIWOOM_GUIDE_STEPS,
  KIWOOM_SECURITY_POINTS,
  type KiwoomMockupId,
} from '../../lib/kiwoomGuide'

interface Props {
  initialStep?: number
  initialView?: 'guide' | 'qa'
  configured?: boolean
  serverIp?: string | null
  onClose: () => void
  onFocusField: () => void
}

function KiwoomStepMockup({ mockup, serverIp }: { mockup: KiwoomMockupId; serverIp?: string | null }) {
  switch (mockup) {
    case 'kiwoom-portal-login':
      return (
        <div className="ng-mock kiwoom">
          <div className="ng-mock-bar">
            <span className="ng-app-dot kiwoom" />
            <b>openapi.kiwoom.com</b>
            <span className="ng-bar-sub">키움 REST API 포털</span>
          </div>
          <div className="ng-sl-body">
            <div className="ng-sl-head">
              <b>키움증권 REST API</b>
              <span className="ng-btn-pill kiwoom highlight">① 서비스 이용신청</span>
            </div>
            <div className="ng-dc-opt highlight">
              <b>② 실전 계좌 또는 상시모의투자 선택</b>
              <div className="muted small">윈도우 OCX·공인인증서 없이 리눅스·웹에서 바로 연동!</div>
            </div>
          </div>
        </div>
      )

    case 'kiwoom-register-ip':
      return (
        <div className="ng-mock kiwoom">
          <div className="ng-mock-bar">
            <span className="ng-app-dot kiwoom" />
            <b>키움 포털 · 허용 IP 등록 관리</b>
            <span className="ng-bar-sub">최대 10개 등록</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-token-box highlight">
              <div className="ng-url-cut">
                <div className="muted small">허용 IP 주소 입력</div>
                <code>{serverIp || '이 서비스 서버의 공인 IP'}</code>
              </div>
              <span className="ng-btn-pill kiwoom highlight">+ IP 등록</span>
              <span className="ng-callout">👈 ① 붙여넣고 저장!</span>
            </div>
            <div className="ng-tg-row">
              <span className="status-badge approved">등록됨 ✓</span>
              <code className="small">{serverIp || '예: 123.45.67.89'}</code>
              <span className="muted small" style={{ marginLeft: 'auto' }}>MightyTrade 서버</span>
            </div>
          </div>
        </div>
      )

    case 'kiwoom-copy-keys':
      return (
        <div className="ng-mock kiwoom">
          <div className="ng-mock-bar">
            <span className="ng-app-dot kiwoom" />
            <b>키움 포털 · 앱 키 관리</b>
            <span className="ng-bar-sub">실전 / 모의 키 구분</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-dc-wh-top">
              <span className="ng-btn-pill kiwoom">실전투자 (api.kiwoom.com)</span>
              <span className="ng-btn-pill">모의투자 (mockapi.kiwoom.com)</span>
            </div>
            <div className="ng-token-box highlight">
              <div className="ng-url-cut">
                <div className="muted small">App Key</div>
                <code>PS9xK2mN8aQ7bW…</code>
              </div>
              <span className="ng-btn-pill primary">복사</span>
              <span className="ng-callout">👈 ① 복사</span>
            </div>
            <div className="ng-token-box highlight">
              <div className="ng-url-cut">
                <div className="muted small">Secret Key</div>
                <code>SK7zL9pR••••••••••••</code>
              </div>
              <span className="ng-btn-pill primary">복사</span>
              <span className="ng-callout">👈 ② 복사</span>
            </div>
          </div>
        </div>
      )

    case 'kiwoom-paste-otp':
      return (
        <div className="ng-mock mt">
          <div className="ng-mock-bar">
            <span className="ng-app-dot mt" />
            <b>MightyTrade · 설정</b>
            <span className="ng-bar-sub">키움증권 REST API 등록</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-dc-wh-top">
              <span className="ng-btn-pill kiwoom highlight">① 실전 / 모의 선택</span>
              <span className="ng-callout">👈 발급한 키와 맞추기!</span>
            </div>
            <div className="ng-mini-field highlight">
              <span>② App Key &amp; Secret Key</span>
              <code>PS9xK2mN… / ••••••••••••</code>
            </div>
            <div className="ng-mini-field highlight">
              <span>③ OTP 코드 (스마트폰 인증 앱 6자리)</span>
              <code>3 9 1 7 0 4</code>
            </div>
            <div className="ng-mini-btns">
              <span className="ng-btn-pill primary highlight">④ 확인 후 저장</span>
              <span className="ng-callout">👈 OAuth2(au10001) 자동 검증!</span>
            </div>
          </div>
        </div>
      )

    case 'kiwoom-ready-features':
      return (
        <div className="ng-mock kiwoom">
          <div className="ng-mock-bar">
            <span className="ng-app-dot kiwoom" />
            <b>키움증권 연동 완료!</b>
            <span className="ng-bar-sub">전 기능 바로 사용</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-dc-opt highlight">📡 <b>실시간 호가·체결</b> — 키움 WebSocket(`0B`/`0D`) 실시간 중계</div>
            <div className="ng-dc-opt highlight">🔍 <b>영웅문 조건검색식</b> — 퀀트 랩 [종목 발굴]에서 내 HTS 조건식 즉시 조회</div>
            <div className="ng-dc-opt">🤖 <b>국내·미국 6종 자동매매 봇</b> — 공식 모의투자 연습 후 실전 전환 가능</div>
          </div>
        </div>
      )
  }
}

export default function KiwoomGuideModal({
  initialStep = 0,
  initialView = 'guide',
  configured = false,
  serverIp = null,
  onClose,
  onFocusField,
}: Props) {
  const [view, setView] = useState<'guide' | 'qa'>(initialView)
  const [stepIdx, setStepIdx] = useState(initialStep)
  const [copied, setCopied] = useState(false)

  const safeStep = Math.min(Math.max(stepIdx, 0), KIWOOM_GUIDE_STEPS.length - 1)
  const step = KIWOOM_GUIDE_STEPS[safeStep]
  const isFirst = safeStep === 0
  const isLast = safeStep === KIWOOM_GUIDE_STEPS.length - 1

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
      if (view !== 'guide') return
      if (e.key === 'ArrowRight' && safeStep < KIWOOM_GUIDE_STEPS.length - 1) setStepIdx(safeStep + 1)
      if (e.key === 'ArrowLeft' && safeStep > 0) setStepIdx(safeStep - 1)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, safeStep, view])

  const copyServerIp = async () => {
    if (!serverIp) return
    try {
      await navigator.clipboard.writeText(serverIp)
    } catch {
      const ta = document.createElement('textarea')
      ta.value = serverIp
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
    }
    setCopied(true)
    setTimeout(() => setCopied(false), 1800)
  }

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal ng-modal" role="dialog" aria-modal="true" aria-labelledby="kw-guide-title">
        <div className="account-head">
          <h3 id="kw-guide-title">🟣 키움증권 REST API 연결 화면별 도움말</h3>
          <div className="spacer" />
          <button type="button" title="도움말 창을 닫습니다" onClick={onClose}>닫기</button>
        </div>

        <p className="muted small" style={{ margin: 0, lineHeight: 1.5 }}>
          키움 공식 REST API 포털에서 <b>이용 신청 → 허용 IP 등록 → App/Secret Key 복사 → OTP 확인</b>까지
          5단계 화면을 보면서 약 3분 안에 국내·미국 계좌(실전/모의)를 연결할 수 있습니다.
        </p>

        <div className="ng-tabs">
          <button
            type="button"
            className={view === 'guide' ? 'active' : ''}
            onClick={() => setView('guide')}
          >
            <span>🟣 5단계 화면 따라 하기</span>
            {configured && <span className="ng-conn-dot" title="연결됨">✓</span>}
          </button>
          <button
            type="button"
            className={view === 'qa' ? 'active' : ''}
            onClick={() => setView('qa')}
          >
            🛡️ 보안 · 토스 vs 키움 Q&amp;A
          </button>
        </div>

        {view === 'qa' ? (
          <div className="ng-preview-wrap">
            <div className="ng-channel-banner">
              <b>🔐 키움 REST API 보안 · 특화 기능</b>
              <span className="muted small">
                공인인증서 없이 OAuth2 + 브라우저 이중 암호화 + OTP 잠금으로 안전하게 연동하며, 공식 모의투자도 지원합니다.
              </span>
            </div>
            <div className="ng-toss-sec-grid">
              {KIWOOM_SECURITY_POINTS.map((p) => (
                <div key={p.title} className="ng-preview-card">
                  <div className="ng-preview-title">{p.icon} {p.title}</div>
                  <p className="muted small" style={{ margin: 0, lineHeight: 1.5 }}>{p.desc}</p>
                </div>
              ))}
            </div>
            <div className="ng-toss-faq">
              {KIWOOM_FAQ_ITEMS.map((item) => (
                <div key={item.q} className="ng-preview-card">
                  <div className="ng-preview-title">Q. {item.q}</div>
                  <p className="small" style={{ margin: 0, color: '#c9d1d9', lineHeight: 1.55 }}>{item.a}</p>
                  {item.stepIndex != null && (
                    <button
                      type="button"
                      className="link"
                      style={{ alignSelf: 'flex-start', paddingLeft: 0 }}
                      onClick={() => {
                        setStepIdx(item.stepIndex!)
                        setView('guide')
                      }}
                    >
                      → {item.stepIndex + 1}단계 화면 바로 보기
                    </button>
                  )}
                </div>
              ))}
            </div>
            <div className="modal-actions" style={{ width: '100%' }}>
              <button type="button" onClick={() => setView('guide')}>← 5단계 따라 하기로 돌아가기</button>
              <button type="button" className="primary" onClick={onClose}>확인</button>
            </div>
          </div>
        ) : (
          <>
            <div className="ng-channel-banner">
              <div className="ng-banner-top">
                <b>🟣 키움증권 REST API 준비물</b>
                <span className="ng-pill">실전 + 공식 모의 지원</span>
                <span className="ng-pill muted">⏱ 약 3분</span>
                <span className="ng-pill muted">국내 + 미국</span>
              </div>
              <span className="small">
                필요한 값:
                <code className="ng-need-tag">허용 IP 등록</code>
                <code className="ng-need-tag">App Key</code>
                <code className="ng-need-tag">Secret Key</code>
                <code className="ng-need-tag">OTP 6자리</code>
              </span>
            </div>

            <div className="ng-stepper" role="tablist" aria-label="키움증권 연결 단계">
              {KIWOOM_GUIDE_STEPS.map((s, idx) => (
                <button
                  key={s.id}
                  type="button"
                  role="tab"
                  aria-selected={idx === safeStep}
                  className={`ng-step-pill ${idx === safeStep ? 'active' : idx < safeStep ? 'done' : ''}`}
                  title={`${idx + 1}단계: ${s.shortTitle}`}
                  onClick={() => setStepIdx(idx)}
                >
                  <span className="ng-step-num">{idx < safeStep ? '✓' : idx + 1}</span>
                  <span className="ng-step-lbl">{s.shortTitle}</span>
                </button>
              ))}
            </div>

            <div className="ng-stage">
              <div className="ng-stage-head">
                <h4>{step.title}</h4>
                <span className="muted small">{safeStep + 1} / {KIWOOM_GUIDE_STEPS.length}</span>
              </div>
              <p className="small ng-stage-summary">{step.summary}</p>

              <KiwoomStepMockup mockup={step.mockup} serverIp={serverIp} />

              <ol className="ng-action-list small">
                {step.actions.map((line, i) => (
                  <li key={i}>{line}</li>
                ))}
              </ol>

              {(step.mockup === 'kiwoom-register-ip' || step.links?.length) && (
                <div className="ng-tools-row">
                  {step.mockup === 'kiwoom-register-ip' && serverIp && (
                    <button
                      type="button"
                      className="ng-copy-btn"
                      title={`키움 포털 허용 IP 관리에 붙여넣을 서버 IP(${serverIp})를 복사합니다`}
                      onClick={() => void copyServerIp()}
                    >
                      {copied ? '✓ 서버 IP 복사됨!' : `📋 이 서버 IP 복사: ${serverIp}`}
                    </button>
                  )}
                  {step.mockup === 'kiwoom-register-ip' && !serverIp && (
                    <span className="muted small">
                      ※ 로컬(내 PC) 실행 중이라면 내 PC의 공인 IP를, 운영 서버 접속 중이라면 도메인 IP를 등록하세요.
                    </span>
                  )}
                  {step.links?.map((lk) => (
                    <a
                      key={lk.url}
                      href={lk.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="ext-link"
                      title="새 창에서 키움 REST API 공식 포털을 엽니다"
                    >
                      {lk.label}
                    </a>
                  ))}
                </div>
              )}

              {step.tip && (
                <div className="ng-tip small">
                  <b>💡 팁:</b> {step.tip}
                </div>
              )}
              {step.caution && (
                <div className="guide-caution small">
                  <b>⚠️ 주의:</b> {step.caution}
                </div>
              )}
            </div>

            <div className="ng-footer">
              <button
                type="button"
                disabled={isFirst}
                title="이전 단계 화면을 봅니다 (키보드 ←)"
                onClick={() => setStepIdx(safeStep - 1)}
              >
                ← 이전
              </button>
              <div className="spacer" />
              {!isLast ? (
                <button
                  type="button"
                  className="primary"
                  title="다음 단계 화면으로 넘어갑니다 (키보드 →)"
                  onClick={() => setStepIdx(safeStep + 1)}
                >
                  다음 단계 ({safeStep + 2}/{KIWOOM_GUIDE_STEPS.length}) →
                </button>
              ) : (
                <button
                  type="button"
                  className="primary"
                  title="도움말을 닫고 키움증권 App Key 입력 칸으로 바로 이동합니다"
                  onClick={() => {
                    onClose()
                    onFocusField()
                  }}
                >
                  🟣 키움 키 입력창으로 바로 이동 →
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  )
}
