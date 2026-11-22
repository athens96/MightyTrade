import { useEffect, useState } from 'react'
import {
  NOTIFY_CHANNEL_GUIDES,
  NOTIFY_CHANNEL_LIST,
  SAMPLE_ALERT_PREVIEWS,
  type GuideMockupId,
  type NotifyChannelId,
} from '../../lib/notificationGuides'

interface Props {
  initialChannel?: NotifyChannelId
  initialStep?: number
  initialView?: 'guide' | 'preview'
  configured?: { telegram: boolean; discord: boolean; slack: boolean }
  onClose: () => void
  onFocusField: (channel: NotifyChannelId) => void
}

function StepMockup({ mockup }: { mockup: GuideMockupId }) {
  switch (mockup) {
    case 'tg-search-botfather':
      return (
        <div className="ng-mock tg">
          <div className="ng-mock-bar">
            <span className="ng-app-dot tg" />
            <b>Telegram</b>
            <span className="ng-bar-sub">대화 검색</span>
          </div>
          <div className="ng-tg-search">
            <span>🔍</span>
            <code>@BotFather</code>
          </div>
          <div className="ng-tg-list">
            <div className="ng-tg-row highlight">
              <div className="ng-avatar tg">BF</div>
              <div className="ng-tg-row-body">
                <div>
                  <b>BotFather</b> <span className="ng-verified" title="공식 인증">✓</span>
                  <span className="ng-callout">👈 파란 체크 확인!</span>
                </div>
                <span className="muted small">@BotFather · 공식 봇 생성 도우미</span>
              </div>
            </div>
            <div className="ng-tg-row dimmed">
              <div className="ng-avatar">?</div>
              <div className="ng-tg-row-body">
                <div><span>BotFather_helper</span></div>
                <span className="muted small">체크 없는 유사 계정 주의</span>
              </div>
            </div>
          </div>
          <div className="ng-tg-bottom">
            <span className="ng-btn-pill highlight">시작 (START)</span>
            <span className="ng-callout">👈 대화방 열고 클릭</span>
          </div>
        </div>
      )

    case 'tg-newbot-token':
      return (
        <div className="ng-mock tg">
          <div className="ng-mock-bar">
            <span className="ng-app-dot tg" />
            <b>BotFather</b> <span className="ng-verified">✓</span>
          </div>
          <div className="ng-chat">
            <div className="ng-msg me highlight-soft">
              <code>/newbot</code>
              <span className="ng-callout">👈 ① 입력</span>
            </div>
            <div className="ng-msg bot">Alright, a new bot. How are we going to call it?</div>
            <div className="ng-msg me">마이티트레이드 알림</div>
            <div className="ng-msg bot">Good. Now let&apos;s choose a username (must end in <b>bot</b>).</div>
            <div className="ng-msg me">my_mightytrade_bot</div>
            <div className="ng-msg bot">
              <div>Done! Use this token to access the HTTP API:</div>
              <div className="ng-token-box highlight">
                <code>7412345678:AAH9xK_sampleTokenValue99</code>
                <span className="ng-callout">👈 ② 이 줄 전체 복사!</span>
              </div>
            </div>
          </div>
        </div>
      )

    case 'tg-start-mybot':
      return (
        <div className="ng-mock tg">
          <div className="ng-mock-bar">
            <span className="ng-app-dot tg" />
            <b>마이티트레이드 알림</b>
            <span className="ng-bar-sub">@my_mightytrade_bot</span>
          </div>
          <div className="ng-chat">
            <div className="ng-msg bot">
              <div>방금 만든 내 알림 봇의 채팅방입니다.</div>
              <div className="muted small">아래 [시작]을 눌러야 봇이 나에게 알림을 보낼 수 있어요.</div>
            </div>
            <div className="ng-msg me">
              <code>/start</code>
            </div>
            <div className="ng-msg me">안녕</div>
          </div>
          <div className="ng-tg-bottom">
            <span className="ng-btn-pill highlight">시작 (START)</span>
            <span className="ng-callout">👈 필수! 꼭 한 번 누르기</span>
          </div>
        </div>
      )

    case 'tg-get-chatid':
      return (
        <div className="ng-mock tg">
          <div className="ng-mock-bar">
            <span className="ng-app-dot tg" />
            <b>userinfobot</b>
            <span className="ng-bar-sub">@userinfobot</span>
          </div>
          <div className="ng-chat">
            <div className="ng-msg me"><code>/start</code></div>
            <div className="ng-msg bot">
              <div>@my_telegram_id</div>
              <div className="ng-token-box highlight">
                <span>Id: </span>
                <b>6123456789</b>
                <span className="ng-callout">👈 이 숫자 복사!</span>
              </div>
              <div className="muted small">First: 홍길동 · Lang: ko</div>
            </div>
          </div>
          <div className="ng-mock-foot muted small">
            또는 브라우저에 <code>getUpdates</code> 주소 입력 시 <code>&quot;chat&quot;:{'{'}&quot;id&quot;: 6123456789{'}'}</code> 확인
          </div>
        </div>
      )

    case 'tg-paste-settings':
      return (
        <div className="ng-mock mt">
          <div className="ng-mock-bar">
            <span className="ng-app-dot mt" />
            <b>MightyTrade · 설정</b>
            <span className="ng-bar-sub">실시간 매매 알림</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-mini-field highlight">
              <span>텔레그램 Bot Token</span>
              <code>7412345678:AAH9xK_••••••</code>
            </div>
            <div className="ng-mini-field highlight">
              <span>텔레그램 Chat ID</span>
              <code>6123456789</code>
            </div>
            <div className="ng-mini-btns">
              <span className="ng-btn-pill primary">① 알림 설정 저장</span>
              <span className="ng-btn-pill highlight">② 테스트 알림 보내기</span>
            </div>
          </div>
        </div>
      )

    case 'dc-create-server':
      return (
        <div className="ng-mock dc">
          <div className="ng-mock-bar">
            <span className="ng-app-dot dc" />
            <b>Discord</b>
            <span className="ng-bar-sub">나만의 알림 서버 만들기</span>
          </div>
          <div className="ng-dc-layout">
            <div className="ng-dc-rail">
              <div className="ng-dc-icon">D</div>
              <div className="ng-dc-icon active">MT</div>
              <div className="ng-dc-icon plus highlight">+</div>
            </div>
            <div className="ng-dc-card">
              <b>서버 만들기</b>
              <div className="ng-dc-opt highlight">① 직접 만들기 → 나와 친구들을 위한 서버</div>
              <div className="ng-dc-opt">② 서버 이름: <code>내 주식 알림방</code></div>
              <div className="ng-dc-opt">③ 텍스트 채널 <code># 매매-알림</code> 준비 완료</div>
            </div>
          </div>
        </div>
      )

    case 'dc-channel-integrations':
      return (
        <div className="ng-mock dc">
          <div className="ng-mock-bar">
            <span className="ng-app-dot dc" />
            <b>Discord · 채널 설정</b>
          </div>
          <div className="ng-dc-layout">
            <div className="ng-dc-channels">
              <div className="muted small">채팅 채널</div>
              <div className="ng-dc-ch active">
                <span># 매매-알림</span>
                <span className="ng-gear highlight">⚙️</span>
              </div>
              <span className="ng-callout">👈 ① 톱니바퀴 클릭</span>
            </div>
            <div className="ng-dc-menu">
              <div className="ng-dc-menu-item">일반</div>
              <div className="ng-dc-menu-item">권한</div>
              <div className="ng-dc-menu-item highlight">연동 (Integrations) 👈 ②</div>
              <div className="ng-dc-menu-sub">└ 웹후크 (Webhooks) 클릭</div>
            </div>
          </div>
        </div>
      )

    case 'dc-copy-webhook':
      return (
        <div className="ng-mock dc">
          <div className="ng-mock-bar">
            <span className="ng-app-dot dc" />
            <b>Discord · 연동 → 웹후크</b>
          </div>
          <div className="ng-dc-wh">
            <div className="ng-dc-wh-top">
              <span className="ng-btn-pill dc">① 새 웹후크</span>
              <span className="muted small">채널: #매매-알림</span>
            </div>
            <div className="ng-dc-wh-card">
              <div className="ng-dc-wh-head">
                <div className="ng-avatar dc">MT</div>
                <div>
                  <b>마이티트레이드 알림</b>
                  <div className="muted small">https://discord.com/api/webhooks/123…</div>
                </div>
              </div>
              <div className="ng-dc-wh-actions">
                <span className="ng-btn-pill highlight">② 웹후크 URL 복사</span>
                <span className="ng-callout">👈 복사 후 설정에 붙여넣기!</span>
              </div>
            </div>
          </div>
        </div>
      )

    case 'dc-paste-settings':
      return (
        <div className="ng-mock mt">
          <div className="ng-mock-bar">
            <span className="ng-app-dot mt" />
            <b>MightyTrade · 설정</b>
            <span className="ng-bar-sub">디스코드 연결</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-mini-field highlight">
              <span>디스코드 Webhook URL</span>
              <code>https://discord.com/api/webhooks/123456/••••</code>
            </div>
            <div className="ng-mini-btns">
              <span className="ng-btn-pill primary">① 알림 설정 저장</span>
              <span className="ng-btn-pill highlight">② 테스트 알림 보내기</span>
            </div>
          </div>
        </div>
      )

    case 'sl-create-app':
      return (
        <div className="ng-mock sl">
          <div className="ng-mock-bar">
            <span className="ng-app-dot sl" />
            <b>api.slack.com/apps</b>
            <span className="ng-bar-sub">Slack API</span>
          </div>
          <div className="ng-sl-body">
            <div className="ng-sl-head">
              <b>Your Apps</b>
              <span className="ng-btn-pill sl highlight">① Create New App</span>
            </div>
            <div className="ng-sl-modal">
              <div className="ng-sl-opt highlight">
                <b>② From scratch (처음부터 만들기)</b>
                <span className="muted small">간단한 알림용 웹훅 앱을 바로 만듭니다</span>
              </div>
              <div className="ng-sl-opt dimmed">
                <span>From a manifest</span>
              </div>
            </div>
          </div>
        </div>
      )

    case 'sl-app-name':
      return (
        <div className="ng-mock sl">
          <div className="ng-mock-bar">
            <span className="ng-app-dot sl" />
            <b>Name app &amp; choose workspace</b>
          </div>
          <div className="ng-mini-form">
            <div className="ng-mini-field highlight">
              <span>① App Name</span>
              <code>MightyTrade 알림</code>
            </div>
            <div className="ng-mini-field highlight">
              <span>② Pick a workspace</span>
              <code>내 투자 워크스페이스 ▾</code>
            </div>
            <div className="ng-mini-btns">
              <span className="ng-btn-pill sl highlight">③ Create App</span>
            </div>
          </div>
        </div>
      )

    case 'sl-enable-webhooks':
      return (
        <div className="ng-mock sl">
          <div className="ng-mock-bar">
            <span className="ng-app-dot sl" />
            <b>Slack App Settings</b>
          </div>
          <div className="ng-dc-layout">
            <div className="ng-dc-menu">
              <div className="muted small">Features</div>
              <div className="ng-dc-menu-item highlight">① Incoming Webhooks</div>
              <div className="ng-dc-menu-item">Slash Commands</div>
              <div className="ng-dc-menu-item">OAuth &amp; Permissions</div>
            </div>
            <div className="ng-dc-card">
              <b>Incoming Webhooks</b>
              <div className="ng-sl-toggle-row highlight">
                <span>Activate Incoming Webhooks</span>
                <span className="ng-toggle-on">On</span>
              </div>
              <span className="ng-callout">👈 ② 스위치를 On 으로 켜기!</span>
            </div>
          </div>
        </div>
      )

    case 'sl-add-channel-copy':
      return (
        <div className="ng-mock sl">
          <div className="ng-mock-bar">
            <span className="ng-app-dot sl" />
            <b>Webhook URLs for Your Workspace</b>
          </div>
          <div className="ng-sl-body">
            <div className="ng-dc-wh-top">
              <span className="ng-btn-pill sl">① Add New Webhook to Workspace</span>
              <span className="muted small">채널 #주식-알림 선택 → 허용</span>
            </div>
            <div className="ng-token-box highlight">
              <code className="ng-url-cut">https://hooks.slack.com/services/T01…/B09…/xYz</code>
              <span className="ng-btn-pill primary">② Copy</span>
            </div>
          </div>
        </div>
      )

    case 'sl-paste-settings':
      return (
        <div className="ng-mock mt">
          <div className="ng-mock-bar">
            <span className="ng-app-dot mt" />
            <b>MightyTrade · 설정</b>
            <span className="ng-bar-sub">슬랙 연결</span>
          </div>
          <div className="ng-mini-form">
            <div className="ng-mini-field highlight">
              <span>슬랙 Webhook URL</span>
              <code>https://hooks.slack.com/services/T01…/••••</code>
            </div>
            <div className="ng-mini-btns">
              <span className="ng-btn-pill primary">① 알림 설정 저장</span>
              <span className="ng-btn-pill highlight">② 테스트 알림 보내기</span>
            </div>
          </div>
        </div>
      )
  }
}

export default function NotificationGuideModal({
  initialChannel = 'telegram',
  initialStep = 0,
  initialView = 'guide',
  configured,
  onClose,
  onFocusField,
}: Props) {
  const [view, setView] = useState<'guide' | 'preview'>(initialView)
  const [channel, setChannel] = useState<NotifyChannelId>(initialChannel)
  const [stepIdx, setStepIdx] = useState(initialStep)
  const [copiedKey, setCopiedKey] = useState<string | null>(null)

  const guide = NOTIFY_CHANNEL_GUIDES[channel]
  const safeStep = Math.min(Math.max(stepIdx, 0), guide.steps.length - 1)
  const step = guide.steps[safeStep]
  const isFirst = safeStep === 0
  const isLast = safeStep === guide.steps.length - 1

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
      if (view !== 'guide') return
      if (e.key === 'ArrowRight' && safeStep < guide.steps.length - 1) setStepIdx(safeStep + 1)
      if (e.key === 'ArrowLeft' && safeStep > 0) setStepIdx(safeStep - 1)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [guide.steps.length, onClose, safeStep, view])

  const switchChannel = (next: NotifyChannelId) => {
    setView('guide')
    setChannel(next)
    setStepIdx(0)
  }

  const copyText = async (key: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      setCopiedKey(key)
      setTimeout(() => setCopiedKey((cur) => (cur === key ? null : cur)), 1800)
    } catch {
      // Fallback for non-HTTPS local testing
      const ta = document.createElement('textarea')
      ta.value = text
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
      setCopiedKey(key)
      setTimeout(() => setCopiedKey((cur) => (cur === key ? null : cur)), 1800)
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal ng-modal" role="dialog" aria-modal="true" aria-labelledby="ng-title">
        <div className="account-head">
          <h3 id="ng-title">📖 실시간 알림 · 일일 브리핑 연결 도움말</h3>
          <div className="spacer" />
          <button type="button" title="도움말 창을 닫습니다" onClick={onClose}>닫기</button>
        </div>

        <p className="muted small" style={{ margin: 0, lineHeight: 1.5 }}>
          쓰시는 메신저를 고르면 <b>화면 하나하나</b> 따라 하면서 1~2분 안에 연결할 수 있습니다.
          여러 개를 동시에 연결하면 모든 채널로 알림이 함께 도착합니다.
        </p>

        {/* Channel switcher + preview tab */}
        <div className="ng-tabs">
          {NOTIFY_CHANNEL_LIST.map((ch) => {
            const isConn = configured?.[ch.id]
            return (
              <button
                key={ch.id}
                type="button"
                className={view === 'guide' && channel === ch.id ? 'active' : ''}
                title={`${ch.name} 연결 화면별 따라 하기 (${ch.duration})`}
                onClick={() => switchChannel(ch.id)}
              >
                <span>{ch.emoji} {ch.name}</span>
                {isConn && <span className="ng-conn-dot" title="연결됨">✓</span>}
              </button>
            )
          })}
          <button
            type="button"
            className={view === 'preview' ? 'active' : ''}
            title="연결을 마친 뒤 실제로 폰·PC 메신저에 도착하는 알림과 일일 브리핑 모양을 미리 봅니다"
            onClick={() => setView('preview')}
          >
            📊 알림 예시 보기
          </button>
        </div>

        {view === 'preview' ? (
          <div className="ng-preview-wrap">
            <div className="ng-channel-banner">
              <b>실제로 메신저에 이렇게 도착해요</b>
              <span className="muted small">
                주문·체결·서킷브레이커 긴급 알림과 장 마감 직후 일일 성적표 브리핑을 설정에서 원하는 항목만 골라 받을 수 있습니다.
              </span>
            </div>
            <div className="ng-preview-grid">
              {SAMPLE_ALERT_PREVIEWS.map((item) => (
                <div key={item.kind} className="ng-preview-card">
                  <div className="ng-preview-head">
                    <span className="cat-badge">{item.badge}</span>
                  </div>
                  <div className="ng-preview-bubble">
                    <div className="ng-preview-title">{item.title}</div>
                    {item.lines.map((ln, i) => (
                      <div key={i}>{ln}</div>
                    ))}
                  </div>
                  <p className="muted small" style={{ margin: 0 }}>{item.note}</p>
                </div>
              ))}
            </div>
            <div className="modal-actions" style={{ width: '100%' }}>
              <button type="button" onClick={() => setView('guide')}>← 연결 방법 보러 가기</button>
              <button type="button" className="primary" onClick={onClose}>확인</button>
            </div>
          </div>
        ) : (
          <>
            {/* Channel summary banner */}
            <div className="ng-channel-banner">
              <div className="ng-banner-top">
                <b>{guide.emoji} {guide.name} 연결 방법</b>
                <span className="ng-pill">{guide.badge}</span>
                <span className="ng-pill muted">⏱ {guide.duration}</span>
                <span className="ng-pill muted">{guide.difficulty}</span>
              </div>
              <span className="muted small">{guide.bestFor}</span>
              <span className="small">
                필요한 값: {guide.neededFields.map((f) => <code key={f} className="ng-need-tag">{f}</code>)}
              </span>
            </div>

            {/* Step pills */}
            <div className="ng-stepper" role="tablist" aria-label="연결 단계">
              {guide.steps.map((s, idx) => (
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

            {/* Active step card with visual mockup */}
            <div className="ng-stage">
              <div className="ng-stage-head">
                <h4>{step.title}</h4>
                <span className="muted small">{safeStep + 1} / {guide.steps.length}</span>
              </div>
              <p className="small ng-stage-summary">{step.summary}</p>

              <StepMockup mockup={step.mockup} />

              <ol className="ng-action-list small">
                {step.actions.map((line, i) => (
                  <li key={i}>{line}</li>
                ))}
              </ol>

              {(step.copySnippets?.length || step.links?.length) && (
                <div className="ng-tools-row">
                  {step.copySnippets?.map((sn) => {
                    const key = `${step.id}:${sn.label}`
                    const isCopied = copiedKey === key
                    return (
                      <button
                        key={key}
                        type="button"
                        className="ng-copy-btn"
                        title={`"${sn.value}" 를 클립보드에 복사합니다`}
                        onClick={() => void copyText(key, sn.value)}
                      >
                        {isCopied ? '✓ 복사됨!' : `📋 ${sn.label}: ${sn.value.length > 26 ? `${sn.value.slice(0, 24)}…` : sn.value}`}
                      </button>
                    )
                  })}
                  {step.links?.map((lk) => (
                    <a
                      key={lk.url}
                      href={lk.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="ext-link"
                      title="새 창에서 공식 페이지를 엽니다"
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

            {/* Footer step navigation */}
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
                  다음 단계 ({safeStep + 2}/{guide.steps.length}) →
                </button>
              ) : (
                <button
                  type="button"
                  className="primary"
                  title={`도움말을 닫고 ${guide.name} 입력 칸으로 바로 이동합니다`}
                  onClick={() => {
                    onClose()
                    onFocusField(channel)
                  }}
                >
                  {guide.emoji} {guide.name} 입력창으로 바로 이동 →
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  )
}
