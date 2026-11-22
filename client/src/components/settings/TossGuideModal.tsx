import { useEffect, useState } from 'react'
import {
  TOSS_FAQ_ITEMS,
  TOSS_GUIDE_STEPS,
  TOSS_SECURITY_POINTS,
  type TossMockupId,
} from '../../lib/tossGuide'

interface Props {
  initialStep?: number
  initialView?: 'guide' | 'qa'
  configured?: boolean
  serverIp?: string | null
  onClose: () => void
  onFocusField: () => void
}

function TossStepMockup({ mockup, serverIp }: { mockup: TossMockupId; serverIp?: string | null }) {
  switch (mockup) {
    case 'toss-login-wts':
      return (
        <div className="ng-mock toss">
          <div className="ng-mock-bar">
            <span className="ng-app-dot toss" />
            <b>tossinvest.com</b>
            <span className="ng-bar-sub">토스증권 PC 웹 (WTS)</span>
          </div>
          <div className="ng-sl-body">
            <div className="ng-sl-head">
              <b>토스증권 WTS</b>
              <span className="ng-btn-pill toss highlight">① 로그인</span>
            </div>
            <div className="ng-toss-login-card">
              <div className="ng-toss-qr" aria-hidden="true">QR</div>
              <div className="ng-tg-row-body">
                <div>
                  <b>토스 앱 QR 코드로 10초 로그인</b>
                  <span className="ng-callout">👈 PC 브라우저에서 접속!</span>
                </div>
                <span className="muted small">모바일 토스 앱에는 Open API 메뉴가 없어요</span>
              </div>
            </div>
          </div>
        </div>
      )

    case 'toss-open-api-menu':
      return (
        <div className="ng-mock toss">
          <div className="ng-mock-bar">
            <span className="ng-app-dot toss" />
            <b>토스증권 WTS · 설정</b>
            <span className="ng-bar-sub">⚙️ 설정 → Open API</span>
          </div>
          <div className="ng-dc-layout">
            <div className="ng-dc-menu">
              <div className="muted small">⚙️ 설정 메뉴</div>
              <div className="ng-dc-menu-item">내 정보 · 계좌</div>
              <div className="ng-dc-menu-item">화면 · 알림 설정</div>
              <div className="ng-dc-menu-item highlight">Open API 👈 ① 클릭</div>
            </div>
            <div className="ng-dc-card">
              <b>토스증권 Open API</b>
              <div className="muted small">나만의 프로그램에서 안전하게 시세 조회·주문을 연동합니다.</div>
              <div className="ng-dc-wh-actions" style={{ marginTop: 4 }}>
                <span className="ng-btn-pill toss highlight">② 이용 신청 · 약관 동의</span>
                <span className="ng-callout">👈 처음 1회만!</span>
              </div>
            </div>
          </div>
        </div>
      )

    case 'toss-copy-keys':
      return (
        <div className="ng-mock toss">
          <div className="ng-mock-bar">
            <span className="ng-app-dot toss" />
            <b>Open API · 앱 키 관리</b>
            <span className="ng-bar-sub">키 발급 결과</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-dc-wh-top">
              <span className="ng-btn-pill toss">① 키 발급 완료</span>
              <span className="muted small">창을 닫기 전에 두 값을 모두 복사하세요</span>
            </div>
            <div className="ng-token-box highlight">
              <div className="ng-url-cut">
                <div className="muted small">Client ID</div>
                <code>tsck_live_8a9f2c7e1b4d…</code>
              </div>
              <span className="ng-btn-pill primary">복사</span>
              <span className="ng-callout">👈 ② 복사</span>
            </div>
            <div className="ng-token-box highlight">
              <div className="ng-url-cut">
                <div className="muted small">Client Secret (1회만 표시!)</div>
                <code>tscs_live_9xK2mP••••••••</code>
              </div>
              <span className="ng-btn-pill primary">복사</span>
              <span className="ng-callout">👈 ③ 지금 바로 복사!</span>
            </div>
          </div>
        </div>
      )

    case 'toss-allowed-ip':
      return (
        <div className="ng-mock toss">
          <div className="ng-mock-bar">
            <span className="ng-app-dot toss" />
            <b>Open API · 허용 IP 관리</b>
            <span className="ng-bar-sub">필수 보안 설정</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-token-box highlight">
              <div className="ng-url-cut">
                <div className="muted small">허용 IP 주소 입력</div>
                <code>{serverIp || '이 서비스 서버의 공인 IP'}</code>
              </div>
              <span className="ng-btn-pill toss highlight">+ IP 추가</span>
              <span className="ng-callout">👈 ① 붙여넣고 추가!</span>
            </div>
            <div className="ng-tg-row">
              <span className="status-badge approved">허용됨 ✓</span>
              <code className="small">{serverIp || '예: 123.45.67.89'}</code>
              <span className="muted small" style={{ marginLeft: 'auto' }}>MightyTrade 서버</span>
            </div>
          </div>
          <div className="ng-mock-foot muted small">
            미등록 시 5단계 [확인 후 저장]에서 <code>403 Forbidden</code> 오류가 발생합니다.
          </div>
        </div>
      )

    case 'toss-paste-otp':
      return (
        <div className="ng-mock mt">
          <div className="ng-mock-bar">
            <span className="ng-app-dot mt" />
            <b>MightyTrade · 설정</b>
            <span className="ng-bar-sub">토스증권 키 등록</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-mini-field highlight">
              <span>① Client ID</span>
              <code>tsck_live_8a9f2c7e1b4d…</code>
            </div>
            <div className="ng-mini-field highlight">
              <span>② Client Secret (브라우저 이중 암호화)</span>
              <code>••••••••••••••••••••••</code>
            </div>
            <div className="ng-mini-field highlight">
              <span>③ OTP 코드 (스마트폰 인증 앱 6자리)</span>
              <code>4 8 2 9 1 5</code>
            </div>
            <div className="ng-mini-btns">
              <span className="ng-btn-pill primary highlight">④ 확인 후 저장</span>
              <span className="ng-callout">👈 토스 계좌 연결 자동 점검!</span>
            </div>
          </div>
        </div>
      )
  }
}

export default function TossGuideModal({
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

  const safeStep = Math.min(Math.max(stepIdx, 0), TOSS_GUIDE_STEPS.length - 1)
  const step = TOSS_GUIDE_STEPS[safeStep]
  const isFirst = safeStep === 0
  const isLast = safeStep === TOSS_GUIDE_STEPS.length - 1

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
      if (view !== 'guide') return
      if (e.key === 'ArrowRight' && safeStep < TOSS_GUIDE_STEPS.length - 1) setStepIdx(safeStep + 1)
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
      <div className="modal ng-modal" role="dialog" aria-modal="true" aria-labelledby="tg-toss-title">
        <div className="account-head">
          <h3 id="tg-toss-title">🏦 토스증권 Open API 연결 화면별 도움말</h3>
          <div className="spacer" />
          <button type="button" title="도움말 창을 닫습니다" onClick={onClose}>닫기</button>
        </div>

        <p className="muted small" style={{ margin: 0, lineHeight: 1.5 }}>
          토스증권 PC 웹(WTS)에서 <b>API 키 발급 → 허용 IP 등록 → OTP 검증</b>까지 5단계 화면을 하나씩 따라 하면
          약 3분 안에 안전하게 계좌를 연결할 수 있습니다.
        </p>

        <div className="ng-tabs">
          <button
            type="button"
            className={view === 'guide' ? 'active' : ''}
            title="토스증권 Open API 연결을 5단계 화면으로 따라 합니다"
            onClick={() => setView('guide')}
          >
            <span>🏦 5단계 화면 따라 하기</span>
            {configured && <span className="ng-conn-dot" title="연결됨">✓</span>}
          </button>
          <button
            type="button"
            className={view === 'qa' ? 'active' : ''}
            title="API 키 이중 암호화 방식과 403 오류·Secret 분실 등 자주 막히는 질문 해결법을 봅니다"
            onClick={() => setView('qa')}
          >
            🛡️ 보안 · 자주 막히는 Q&amp;A
          </button>
        </div>

        {view === 'qa' ? (
          <div className="ng-preview-wrap">
            <div className="ng-channel-banner">
              <b>🔐 내 토스증권 API 키는 이렇게 이중으로 보호됩니다</b>
              <span className="muted small">
                비밀 키가 평문으로 돌아다니지 않도록 브라우저 암호화 전송 + 서버 암호화 보관 + OTP 잠금을 함께 적용합니다.
              </span>
            </div>
            <div className="ng-toss-sec-grid">
              {TOSS_SECURITY_POINTS.map((p) => (
                <div key={p.title} className="ng-preview-card">
                  <div className="ng-preview-title">{p.icon} {p.title}</div>
                  <p className="muted small" style={{ margin: 0, lineHeight: 1.5 }}>{p.desc}</p>
                </div>
              ))}
            </div>

            <div className="ng-toss-faq">
              {TOSS_FAQ_ITEMS.map((item) => (
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
                <b>🏦 토스증권 Open API 연결 준비물</b>
                <span className="ng-pill">PC 웹(WTS) 전용</span>
                <span className="ng-pill muted">⏱ 약 3분</span>
                <span className="ng-pill muted">5단계</span>
              </div>
              <span className="small">
                필요한 값:
                <code className="ng-need-tag">Client ID</code>
                <code className="ng-need-tag">Client Secret</code>
                <code className="ng-need-tag">허용 IP 등록</code>
                <code className="ng-need-tag">OTP 6자리</code>
              </span>
            </div>

            <div className="ng-stepper" role="tablist" aria-label="토스증권 연결 단계">
              {TOSS_GUIDE_STEPS.map((s, idx) => (
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
                <span className="muted small">{safeStep + 1} / {TOSS_GUIDE_STEPS.length}</span>
              </div>
              <p className="small ng-stage-summary">{step.summary}</p>

              <TossStepMockup mockup={step.mockup} serverIp={serverIp} />

              <ol className="ng-action-list small">
                {step.actions.map((line, i) => (
                  <li key={i}>{line}</li>
                ))}
              </ol>

              {(step.mockup === 'toss-allowed-ip' || step.links?.length) && (
                <div className="ng-tools-row">
                  {step.mockup === 'toss-allowed-ip' && serverIp && (
                    <button
                      type="button"
                      className="ng-copy-btn"
                      title={`허용 IP 관리에 붙여넣을 이 서비스 서버 IP(${serverIp})를 복사합니다`}
                      onClick={() => void copyServerIp()}
                    >
                      {copied ? '✓ 서버 IP 복사됨!' : `📋 이 서버 IP 복사: ${serverIp}`}
                    </button>
                  )}
                  {step.mockup === 'toss-allowed-ip' && !serverIp && (
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
                      title="새 창에서 토스증권 PC 웹(WTS)을 엽니다"
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
                  다음 단계 ({safeStep + 2}/{TOSS_GUIDE_STEPS.length}) →
                </button>
              ) : (
                <button
                  type="button"
                  className="primary"
                  title="도움말을 닫고 토스증권 Client ID 입력 칸으로 바로 이동합니다"
                  onClick={() => {
                    onClose()
                    onFocusField()
                  }}
                >
                  🏦 토스 키 입력창으로 바로 이동 →
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  )
}
