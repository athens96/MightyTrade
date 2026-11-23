import { useEffect, useRef, useState, type FormEvent } from 'react'
import {
  brokerApi,
  type BrokerKind,
  type BrokersOverview,
  type KiwoomCredentialStatus,
  type NotificationEvents,
  type NotificationSettingsView,
  type TossCredentialStatus,
} from '../api/broker'
import { useAuth } from '../auth/AuthContext'
import { FeatureGuideBox } from '../components/GuideBox'
import KiwoomGuideModal from '../components/settings/KiwoomGuideModal'
import NotificationGuideModal from '../components/settings/NotificationGuideModal'
import TossGuideModal from '../components/settings/TossGuideModal'
import { FEATURE_GUIDES } from '../lib/featureGuides'
import {
  KIWOOM_QUICK_CARDS,
  kiwoomGuideTargetForField,
  type KiwoomFieldAnchor,
} from '../lib/kiwoomGuide'
import {
  guideTargetForField,
  NOTIFY_CHANNEL_LIST,
  type NotifyChannelId,
} from '../lib/notificationGuides'
import {
  TOSS_QUICK_CARDS,
  tossGuideTargetForField,
  type TossFieldAnchor,
} from '../lib/tossGuide'
import { APP_VERSION, fetchHealth, forceReloadApp, isNewerVersion } from '../lib/version'

const DEFAULT_EVENTS: NotificationEvents = {
  order: true,
  fill: true,
  error: true,
  halt: true,
  briefing: true,
}

export default function SettingsPage() {
  const { user } = useAuth()
  const [brokers, setBrokers] = useState<BrokersOverview | null>(null)
  const [brokerTab, setBrokerTab] = useState<BrokerKind>('toss')
  const [switchBusy, setSwitchBusy] = useState(false)
  const [switchMsg, setSwitchMsg] = useState<{ ok: boolean; text: string } | null>(null)

  const [status, setStatus] = useState<TossCredentialStatus | null>(null)
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [otpCode, setOtpCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null)

  const [kwStatus, setKwStatus] = useState<KiwoomCredentialStatus | null>(null)
  const [kwAppKey, setKwAppKey] = useState('')
  const [kwSecretKey, setKwSecretKey] = useState('')
  const [kwAccountNo, setKwAccountNo] = useState('')
  const [kwIsMock, setKwIsMock] = useState(true)
  const [kwOtp, setKwOtp] = useState('')
  const [kwBusy, setKwBusy] = useState(false)
  const [kwMessage, setKwMessage] = useState<{ ok: boolean; text: string } | null>(null)

  const [notify, setNotify] = useState<NotificationSettingsView | null>(null)
  const [nEnabled, setNEnabled] = useState(true)
  const [nPaper, setNPaper] = useState(false)
  const [tgToken, setTgToken] = useState('')
  const [tgChatId, setTgChatId] = useState('')
  const [discordUrl, setDiscordUrl] = useState('')
  const [slackUrl, setSlackUrl] = useState('')
  const [events, setEvents] = useState<NotificationEvents>(DEFAULT_EVENTS)
  const [nBusy, setNBusy] = useState(false)
  const [nMsg, setNMsg] = useState<{ ok: boolean; text: string } | null>(null)
  const [serverVer, setServerVer] = useState<string | null>(null)
  const [guideModal, setGuideModal] = useState<{
    channel: NotifyChannelId
    step: number
    view?: 'guide' | 'preview'
  } | null>(null)
  const [tossGuideModal, setTossGuideModal] = useState<{
    step: number
    view?: 'guide' | 'qa'
  } | null>(null)
  const [kiwoomGuideModal, setKiwoomGuideModal] = useState<{
    step: number
    view?: 'guide' | 'qa'
  } | null>(null)
  const [focusHighlight, setFocusHighlight] = useState<NotifyChannelId | null>(null)
  const [tossFocusPulse, setTossFocusPulse] = useState(false)
  const [kwFocusPulse, setKwFocusPulse] = useState(false)
  const [ipCopied, setIpCopied] = useState(false)

  const tossClientIdRef = useRef<HTMLInputElement | null>(null)
  const kwAppKeyRef = useRef<HTMLInputElement | null>(null)
  const tgTokenRef = useRef<HTMLInputElement | null>(null)
  const discordUrlRef = useRef<HTMLInputElement | null>(null)
  const slackUrlRef = useRef<HTMLInputElement | null>(null)

  const serverIp = brokers?.server_ip ?? status?.server_ip ?? kwStatus?.server_ip ?? null

  const applyBrokersOverview = (ov: BrokersOverview, syncTab = false) => {
    setBrokers(ov)
    setStatus({ ...ov.toss, server_ip: ov.server_ip })
    setKwStatus({ ...ov.kiwoom, server_ip: ov.server_ip })
    if (syncTab) setBrokerTab(ov.active_broker)
  }

  const openTossFieldGuide = (anchor: TossFieldAnchor) => {
    setTossGuideModal({ step: tossGuideTargetForField(anchor), view: 'guide' })
  }

  const openKiwoomFieldGuide = (anchor: KiwoomFieldAnchor) => {
    setKiwoomGuideModal({ step: kiwoomGuideTargetForField(anchor), view: 'guide' })
  }

  const focusTossInput = () => {
    setBrokerTab('toss')
    setTimeout(() => {
      const el = tossClientIdRef.current
      if (!el) return
      el.scrollIntoView({ behavior: 'smooth', block: 'center' })
      setTimeout(() => el.focus(), 180)
      setTossFocusPulse(true)
      setTimeout(() => setTossFocusPulse(false), 2200)
    }, 40)
  }

  const focusKiwoomInput = () => {
    setBrokerTab('kiwoom')
    setTimeout(() => {
      const el = kwAppKeyRef.current
      if (!el) return
      el.scrollIntoView({ behavior: 'smooth', block: 'center' })
      setTimeout(() => el.focus(), 180)
      setKwFocusPulse(true)
      setTimeout(() => setKwFocusPulse(false), 2200)
    }, 40)
  }

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
    setIpCopied(true)
    setTimeout(() => setIpCopied(false), 1800)
  }

  const switchActiveBroker = async (kind: BrokerKind) => {
    setSwitchBusy(true)
    setSwitchMsg(null)
    try {
      const ov = await brokerApi.setActiveBroker(kind)
      applyBrokersOverview(ov, true)
      setSwitchMsg({
        ok: true,
        text: `활성 증권사가 ${kind === 'kiwoom' ? '키움증권' : '토스증권'}으로 전환되었습니다.`,
      })
    } catch (err) {
      setSwitchMsg({ ok: false, text: (err as Error).message })
    } finally {
      setSwitchBusy(false)
    }
  }

  const openFieldGuide = (anchor: 'tg-token' | 'tg-chat-id' | 'discord-url' | 'slack-url') => {
    const t = guideTargetForField(anchor)
    setGuideModal({ channel: t.channel, step: t.stepIndex, view: 'guide' })
  }

  const focusChannelInput = (ch: NotifyChannelId) => {
    const el = ch === 'telegram' ? tgTokenRef.current : ch === 'discord' ? discordUrlRef.current : slackUrlRef.current
    if (!el) return
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
    setTimeout(() => el.focus(), 180)
    setFocusHighlight(ch)
    setTimeout(() => setFocusHighlight((cur) => (cur === ch ? null : cur)), 2200)
  }

  const applyNotify = (v: NotificationSettingsView) => {
    setNotify(v)
    setNEnabled(v.enabled)
    setNPaper(v.paper_notify)
    setTgChatId(v.telegram_chat_id)
    setEvents(v.events)
    setTgToken('')
    setDiscordUrl('')
    setSlackUrl('')
  }

  useEffect(() => {
    brokerApi
      .brokersOverview()
      .then((ov) => applyBrokersOverview(ov, true))
      .catch(() => {
        brokerApi.tossStatus().then(setStatus).catch((e) => setMessage({ ok: false, text: e.message }))
      })
    brokerApi.notificationsStatus().then(applyNotify).catch(() => undefined)
    fetchHealth().then((h) => setServerVer(h.version ?? null)).catch(() => undefined)
  }, [])

  const refreshBrokers = async () => {
    try {
      const ov = await brokerApi.brokersOverview()
      applyBrokersOverview(ov, false)
    } catch {
      // ignore
    }
  }

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (!user) return
    setBusy(true)
    setMessage(null)
    try {
      const next = await brokerApi.saveToss(user.id, {
        client_id: clientId.trim(),
        client_secret: clientSecret.trim(),
        otp_code: otpCode,
      })
      setStatus(next)
      await refreshBrokers()
      setMessage({ ok: true, text: '토스증권 API 키가 확인되어 저장되었습니다.' })
    } catch (err) {
      setMessage({ ok: false, text: (err as Error).message })
    } finally {
      // Don't keep secrets in memory/UI longer than needed.
      setClientSecret('')
      setOtpCode('')
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!confirm('등록된 토스증권 API 키를 삭제할까요?')) return
    try {
      setStatus(await brokerApi.deleteToss())
      await refreshBrokers()
      setMessage({ ok: true, text: '토스증권 키가 삭제되었습니다.' })
    } catch (err) {
      setMessage({ ok: false, text: (err as Error).message })
    }
  }

  const submitKiwoom = async (e: FormEvent) => {
    e.preventDefault()
    if (!user) return
    setKwBusy(true)
    setKwMessage(null)
    try {
      const next = await brokerApi.saveKiwoom(user.id, {
        appkey: kwAppKey.trim(),
        secretkey: kwSecretKey.trim(),
        account_no: kwAccountNo.trim(),
        is_mock: kwIsMock,
        otp_code: kwOtp,
        activate: true,
      })
      setKwStatus(next)
      await refreshBrokers()
      setKwMessage({
        ok: true,
        text: `키움증권(${kwIsMock ? '공식 모의투자' : '실전투자'}) API 키가 확인되어 저장·활성화되었습니다.`,
      })
    } catch (err) {
      setKwMessage({ ok: false, text: (err as Error).message })
    } finally {
      setKwSecretKey('')
      setKwOtp('')
      setKwBusy(false)
    }
  }

  const removeKiwoom = async () => {
    if (!confirm('등록된 키움증권 REST API 키를 삭제할까요?')) return
    try {
      setKwStatus(await brokerApi.deleteKiwoom())
      await refreshBrokers()
      setKwMessage({ ok: true, text: '키움증권 키가 삭제되었습니다.' })
    } catch (err) {
      setKwMessage({ ok: false, text: (err as Error).message })
    }
  }

  return (
    <div className="settings">
      <h2>증권사 API 연결 (토스증권 · 키움증권)</h2>
      <FeatureGuideBox guide={FEATURE_GUIDES.settings} />

      <section className="panel">
        <h3>현재 사용 중인 증권사 선택</h3>
        <p className="muted small" style={{ marginTop: 0 }}>
          토스증권과 키움증권(실전·공식 모의)을 모두 등록해 두고 클릭 한 번으로 활성 증권사를 바꿀 수 있습니다.
        </p>
        <div className="ng-broker-grid">
          <div className={`ng-broker-card ${brokers?.active_broker === 'toss' && status?.configured ? 'active' : ''}`}>
            <div className="ng-channel-card-top">
              <b>🏦 토스증권 (Open API)</b>
              <span className={`status-badge ${status?.configured ? (brokers?.active_broker === 'toss' ? 'approved' : 'in_review') : 'closed'}`}>
                {!status?.configured ? '미연결' : brokers?.active_broker === 'toss' ? '사용 중 ✓' : '연결됨'}
              </span>
            </div>
            <p className="muted small ng-channel-card-desc">
              {status?.configured
                ? `Client ID: ${status.client_id_masked} · 서버 OCO 조건주문 지원`
                : '국내·미국 실시간 시세·주문 및 증권사 서버 OCO 익절/손절 지원'}
            </p>
            <div className="ng-toss-ip-actions">
              <button
                type="button"
                className={brokerTab === 'toss' ? 'active small' : 'small'}
                onClick={() => setBrokerTab('toss')}
              >
                {status?.configured ? '토스 키 관리' : '토스 등록하기'}
              </button>
              {status?.configured && brokers?.active_broker !== 'toss' && (
                <button
                  type="button"
                  className="primary small"
                  disabled={switchBusy}
                  onClick={() => void switchActiveBroker('toss')}
                >
                  토스로 전환
                </button>
              )}
            </div>
          </div>

          <div className={`ng-broker-card ${brokers?.active_broker === 'kiwoom' && kwStatus?.configured ? 'active' : ''}`}>
            <div className="ng-channel-card-top">
              <b>🟣 키움증권 (REST API)</b>
              <span className={`status-badge ${kwStatus?.configured ? (brokers?.active_broker === 'kiwoom' ? 'approved' : 'in_review') : 'closed'}`}>
                {!kwStatus?.configured
                  ? '미연결'
                  : brokers?.active_broker === 'kiwoom'
                    ? kwStatus.is_mock
                      ? '모의 사용 중 ✓'
                      : '실전 사용 중 ✓'
                    : kwStatus.is_mock
                      ? '모의 연결됨'
                      : '실전 연결됨'}
              </span>
            </div>
            <p className="muted small ng-channel-card-desc">
              {kwStatus?.configured
                ? `App Key: ${kwStatus.appkey_masked} · 실시간 WS + 영웅문 조건검색 지원`
                : '국내·미국 실전 + 공식 모의투자 서버, 실시간 WS 호가, 영웅문 조건검색식 연동'}
            </p>
            <div className="ng-toss-ip-actions">
              <button
                type="button"
                className={brokerTab === 'kiwoom' ? 'active small' : 'small'}
                onClick={() => setBrokerTab('kiwoom')}
              >
                {kwStatus?.configured ? '키움 키 관리' : '키움 등록하기'}
              </button>
              {kwStatus?.configured && brokers?.active_broker !== 'kiwoom' && (
                <button
                  type="button"
                  className="primary small"
                  disabled={switchBusy}
                  onClick={() => void switchActiveBroker('kiwoom')}
                >
                  키움으로 전환
                </button>
              )}
            </div>
          </div>
        </div>
        {switchMsg && <p className={switchMsg.ok ? 'ok small' : 'error small'} style={{ marginBottom: 0 }}>{switchMsg.text}</p>}
      </section>

      {brokerTab === 'toss' ? (
        <>
          <section className="panel">
            <h3>🏦 토스증권 현재 상태</h3>
            {!status && <p className="muted">불러오는 중…</p>}
            {status && !status.configured && <p className="muted">등록된 토스증권 키가 없습니다.</p>}
            {status?.configured && (
              <>
                <dl className="kv">
                  <dt>Client ID</dt>
                  <dd><code>{status.client_id_masked}</code></dd>
                  <dt>계좌</dt>
                  <dd>{status.account_seq != null ? `accountSeq ${status.account_seq}` : '-'}</dd>
                  <dt>확인 시각</dt>
                  <dd>{status.verified_at ? new Date(status.verified_at).toLocaleString() : '-'}</dd>
                </dl>
                <button className="danger" title="서버에 암호화되어 저장된 내 토스증권 API 키를 즉시 삭제합니다" onClick={remove}>토스 키 삭제</button>
              </>
            )}
          </section>

          <section className="panel">
            <div className="ng-panel-head">
              <h3>{status?.configured ? '토스증권 키 변경' : '토스증권 키 등록'}</h3>
              <div className="ng-panel-head-btns">
                <button
                  type="button"
                  className="primary small"
                  title="토스증권 PC 웹(WTS) 키 발급부터 허용 IP 등록·OTP 확인까지 5단계 화면을 보면서 따라 합니다"
                  onClick={() => setTossGuideModal({ step: 0, view: 'guide' })}
                >
                  📖 화면 보면서 따라 하기
                </button>
                <button
                  type="button"
                  className="small"
                  title="이중 암호화 보안 원리와 403 오류·Secret 분실 등 자주 막히는 질문 해결법을 봅니다"
                  onClick={() => setTossGuideModal({ step: 0, view: 'qa' })}
                >
                  🛡️ 보안 · Q&amp;A
                </button>
              </div>
            </div>
            <p className="muted small" style={{ marginTop: 4 }}>
              처음 연결하시나요? 아래 3단계 카드를 누르거나 <b>[📖 화면 보면서 따라 하기]</b>를 열면 토스증권 WTS 화면을 보면서 3분 안에 등록할 수 있습니다.
            </p>

            <div className="ng-channel-strip">
              {TOSS_QUICK_CARDS.map((card) => (
                <div key={card.id} className="ng-channel-card">
                  <div className="ng-channel-card-top">
                    <b>{card.title}</b>
                    <span className="ng-pill">{card.stepBadge}</span>
                  </div>
                  <p className="small muted ng-channel-card-desc">{card.desc}</p>
                  <button
                    type="button"
                    className="ng-card-btn"
                    title={`${card.title} 화면별 안내를 바로 엽니다`}
                    onClick={() => setTossGuideModal({ step: card.stepIndex, view: 'guide' })}
                  >
                    해당 화면 보기 →
                  </button>
                </div>
              ))}
            </div>

            {serverIp && (
              <div className="ng-toss-ip-bar small">
                <span>
                  🌐 토스 <b>허용 IP 관리</b>에 넣을 이 서버 IP: <code>{serverIp}</code>
                </span>
                <div className="ng-toss-ip-actions">
                  <button
                    type="button"
                    className="ng-copy-btn"
                    title="토스증권 WTS 허용 IP 관리에 붙여넣을 서버 IP를 복사합니다"
                    onClick={() => void copyServerIp()}
                  >
                    {ipCopied ? '✓ 복사됨!' : '📋 IP 복사'}
                  </button>
                  <button
                    type="button"
                    className="ng-help-link"
                    title="토스증권 WTS 허용 IP 등록 화면을 바로 봅니다"
                    onClick={() => openTossFieldGuide('allowed-ip')}
                  >
                    ? 등록 화면
                  </button>
                </div>
              </div>
            )}

            <form className="form" onSubmit={submit} autoComplete="off">
              <div className="ng-field">
                <div className="ng-label-row">
                  <label htmlFor="toss-client-id" title="토스증권 PC 웹(WTS) → 설정 → Open API에서 발급받은 Client ID를 붙여넣으세요">
                    Client ID
                  </label>
                  <button
                    type="button"
                    className="ng-help-link"
                    title="토스증권 WTS에서 Client ID를 발급·복사하는 화면을 바로 봅니다"
                    onClick={() => openTossFieldGuide('client-id')}
                  >
                    ? 발급 화면 보기
                  </button>
                </div>
                <input
                  id="toss-client-id"
                  ref={tossClientIdRef}
                  className={tossFocusPulse ? 'ng-input-pulse' : ''}
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                  placeholder="tsck_live_…"
                  title="토스증권 Open API에서 발급받은 Client ID(tsck_live_... 등)를 붙여넣으세요"
                  spellCheck={false}
                />
              </div>

              <div className="ng-field">
                <div className="ng-label-row">
                  <label htmlFor="toss-client-secret" title="토스증권 Open API에서 함께 발급받은 비밀 키(Client Secret)를 입력하세요 (암호화되어 전송·저장됩니다)">
                    Client Secret
                  </label>
                  <button
                    type="button"
                    className="ng-help-link"
                    title="Client Secret 1회 노출 주의사항과 재발급 방법을 바로 봅니다"
                    onClick={() => openTossFieldGuide('client-secret')}
                  >
                    ? Secret 복사 주의
                  </button>
                </div>
                <input
                  id="toss-client-secret"
                  className={tossFocusPulse ? 'ng-input-pulse' : ''}
                  type="password"
                  autoComplete="new-password"
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                  placeholder="발급 직후 복사한 Client Secret 붙여넣기"
                  title="토스증권 Open API에서 발급받은 Client Secret을 입력하세요 (암호화되어 전송·저장됩니다)"
                  spellCheck={false}
                />
              </div>

              <div className="ng-field">
                <div className="ng-label-row">
                  <label htmlFor="toss-otp-code" title="본인 확인을 위해 스마트폰 인증 앱(Google Authenticator 등)에 표시된 6자리 숫자를 입력하세요">
                    OTP 코드
                  </label>
                  <button
                    type="button"
                    className="ng-help-link"
                    title="허용 IP 확인 및 OTP 6자리 검증 단계를 바로 봅니다"
                    onClick={() => openTossFieldGuide('otp')}
                  >
                    ? OTP · 저장 확인
                  </button>
                </div>
                <input
                  id="toss-otp-code"
                  className={tossFocusPulse ? 'ng-input-pulse' : ''}
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  value={otpCode}
                  onChange={(e) => setOtpCode(e.target.value.replace(/\D/g, ''))}
                  placeholder="스마트폰 인증 앱 6자리 숫자"
                  title="스마트폰 인증 앱에 표시된 6자리 일회용 숫자를 입력하세요"
                />
              </div>

              <button
                type="submit"
                className="primary"
                disabled={busy || !clientId.trim() || !clientSecret.trim() || otpCode.length !== 6}
                title="입력한 API 키가 정상 작동하는지 토스증권에 먼저 확인한 뒤 암호화해 안전하게 저장합니다"
              >
                {busy ? '토스증권에 확인 중…' : '확인 후 저장'}
              </button>
            </form>
            {message && <p className={message.ok ? 'ok' : 'error'}>{message.text}</p>}
          </section>
        </>
      ) : (
        <>
          <section className="panel">
            <h3>🟣 키움증권 현재 상태</h3>
            {!kwStatus && <p className="muted">불러오는 중…</p>}
            {kwStatus && !kwStatus.configured && <p className="muted">등록된 키움증권 REST API 키가 없습니다.</p>}
            {kwStatus?.configured && (
              <>
                <dl className="kv">
                  <dt>투자 구분</dt>
                  <dd>
                    <span className={`status-badge ${kwStatus.is_mock ? 'in_review' : 'approved'}`}>
                      {kwStatus.is_mock ? '🧪 공식 모의투자 (mockapi.kiwoom.com)' : '💰 실전투자 (api.kiwoom.com)'}
                    </span>
                  </dd>
                  <dt>App Key</dt>
                  <dd><code>{kwStatus.appkey_masked}</code></dd>
                  {kwStatus.account_no_masked && (
                    <>
                      <dt>계좌 메모</dt>
                      <dd><code>{kwStatus.account_no_masked}</code></dd>
                    </>
                  )}
                  <dt>확인 시각</dt>
                  <dd>{kwStatus.verified_at ? new Date(kwStatus.verified_at).toLocaleString() : '-'}</dd>
                </dl>
                <button className="danger" title="서버에 암호화되어 저장된 내 키움증권 REST API 키를 즉시 삭제합니다" onClick={removeKiwoom}>키움 키 삭제</button>
              </>
            )}
          </section>

          <section className="panel">
            <div className="ng-panel-head">
              <h3>{kwStatus?.configured ? '키움증권 REST API 키 변경' : '키움증권 REST API 키 등록'}</h3>
              <div className="ng-panel-head-btns">
                <button
                  type="button"
                  className="primary small"
                  title="키움 REST API 포털 신청부터 허용 IP 등록·App/Secret 복사·OTP 검증까지 5단계 화면을 보면서 따라 합니다"
                  onClick={() => setKiwoomGuideModal({ step: 0, view: 'guide' })}
                >
                  📖 화면 보면서 따라 하기
                </button>
                <button
                  type="button"
                  className="small"
                  title="토스 vs 키움 차이점, 공식 모의투자, 영웅문 조건검색식 Q&A를 봅니다"
                  onClick={() => setKiwoomGuideModal({ step: 0, view: 'qa' })}
                >
                  🛡️ 보안 · Q&amp;A
                </button>
              </div>
            </div>
            <p className="muted small" style={{ marginTop: 4 }}>
              키움 공식 REST API(<code>openapi.kiwoom.com</code>)는 윈도우 프로그램이나 공인인증서 없이 국내·미국 실전과 <b>공식 모의투자</b>까지 바로 연동됩니다.
            </p>

            <div className="ng-channel-strip">
              {KIWOOM_QUICK_CARDS.map((card) => (
                <div key={card.id} className="ng-channel-card">
                  <div className="ng-channel-card-top">
                    <b>{card.title}</b>
                    <span className="ng-pill">{card.stepBadge}</span>
                  </div>
                  <p className="small muted ng-channel-card-desc">{card.desc}</p>
                  <button
                    type="button"
                    className="ng-card-btn"
                    title={`${card.title} 화면별 안내를 바로 엽니다`}
                    onClick={() => setKiwoomGuideModal({ step: card.stepIndex, view: 'guide' })}
                  >
                    해당 화면 보기 →
                  </button>
                </div>
              ))}
            </div>

            {serverIp && (
              <div className="ng-toss-ip-bar kiwoom small">
                <span>
                  🌐 키움 <b>허용 IP 등록 관리</b>에 넣을 이 서버 IP: <code>{serverIp}</code>
                </span>
                <div className="ng-toss-ip-actions">
                  <button
                    type="button"
                    className="ng-copy-btn"
                    title="키움 REST API 포털 허용 IP에 붙여넣을 서버 IP를 복사합니다"
                    onClick={() => void copyServerIp()}
                  >
                    {ipCopied ? '✓ 복사됨!' : '📋 IP 복사'}
                  </button>
                  <button
                    type="button"
                    className="ng-help-link"
                    title="키움 포털 허용 IP 등록 화면을 바로 봅니다"
                    onClick={() => openKiwoomFieldGuide('allowed-ip')}
                  >
                    ? 등록 화면
                  </button>
                </div>
              </div>
            )}

            <form className="form" onSubmit={submitKiwoom} autoComplete="off">
              <div className="ng-field">
                <div className="ng-label-row">
                  <label title="발급받은 키움 REST API 키가 공식 모의투자용인지 실전투자용인지 선택하세요">
                    키움 서버 구분 (실전 / 공식 모의투자)
                  </label>
                  <button
                    type="button"
                    className="ng-help-link"
                    onClick={() => openKiwoomFieldGuide('mock-mode')}
                  >
                    ? 실전 vs 모의 차이
                  </button>
                </div>
                <div className="seg">
                  <button
                    type="button"
                    className={kwIsMock ? 'active' : ''}
                    onClick={() => setKwIsMock(true)}
                  >
                    🧪 공식 모의투자 (mockapi)
                  </button>
                  <button
                    type="button"
                    className={!kwIsMock ? 'active' : ''}
                    onClick={() => setKwIsMock(false)}
                  >
                    💰 실전투자 (api)
                  </button>
                </div>
              </div>

              <div className="ng-field">
                <div className="ng-label-row">
                  <label htmlFor="kw-appkey" title="키움 REST API 포털에서 발급받은 App Key를 붙여넣으세요">
                    App Key
                  </label>
                  <button
                    type="button"
                    className="ng-help-link"
                    onClick={() => openKiwoomFieldGuide('appkey')}
                  >
                    ? 발급 화면 보기
                  </button>
                </div>
                <input
                  id="kw-appkey"
                  ref={kwAppKeyRef}
                  className={kwFocusPulse ? 'ng-input-pulse' : ''}
                  value={kwAppKey}
                  onChange={(e) => setKwAppKey(e.target.value)}
                  placeholder="키움 포털에서 발급받은 App Key 붙여넣기"
                  spellCheck={false}
                />
              </div>

              <div className="ng-field">
                <div className="ng-label-row">
                  <label htmlFor="kw-secretkey" title="키움 REST API 포털에서 함께 발급받은 Secret Key를 입력하세요">
                    Secret Key
                  </label>
                  <button
                    type="button"
                    className="ng-help-link"
                    onClick={() => openKiwoomFieldGuide('secretkey')}
                  >
                    ? 키 복사 주의
                  </button>
                </div>
                <input
                  id="kw-secretkey"
                  className={kwFocusPulse ? 'ng-input-pulse' : ''}
                  type="password"
                  autoComplete="new-password"
                  value={kwSecretKey}
                  onChange={(e) => setKwSecretKey(e.target.value)}
                  placeholder="발급받은 Secret Key 붙여넣기"
                  spellCheck={false}
                />
              </div>

              <div className="ng-field">
                <label htmlFor="kw-acct" title="화면에 표시할 본인 키움 계좌번호 메모입니다 (선택 사항, 암호화 저장)">
                  계좌번호 (선택 · 메모용)
                </label>
                <input
                  id="kw-acct"
                  value={kwAccountNo}
                  onChange={(e) => setKwAccountNo(e.target.value)}
                  placeholder="예: 50123456-10 (생략 가능)"
                  spellCheck={false}
                />
              </div>

              <div className="ng-field">
                <div className="ng-label-row">
                  <label htmlFor="kw-otp" title="스마트폰 인증 앱에 표시된 6자리 일회용 숫자를 입력하세요">
                    OTP 코드
                  </label>
                  <button
                    type="button"
                    className="ng-help-link"
                    onClick={() => openKiwoomFieldGuide('otp')}
                  >
                    ? OTP · 검증 안내
                  </button>
                </div>
                <input
                  id="kw-otp"
                  className={kwFocusPulse ? 'ng-input-pulse' : ''}
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  value={kwOtp}
                  onChange={(e) => setKwOtp(e.target.value.replace(/\D/g, ''))}
                  placeholder="스마트폰 인증 앱 6자리 숫자"
                />
              </div>

              <button
                type="submit"
                className="primary"
                disabled={kwBusy || !kwAppKey.trim() || !kwSecretKey.trim() || kwOtp.length !== 6}
                title="키움 OAuth2(au10001)로 키와 허용 IP가 정상인지 먼저 확인한 뒤 암호화해 저장합니다"
              >
                {kwBusy ? '키움증권에 확인 중…' : `키움 ${kwIsMock ? '모의' : '실전'} 확인 후 저장`}
              </button>
            </form>
            {kwMessage && <p className={kwMessage.ok ? 'ok' : 'error'}>{kwMessage.text}</p>}
          </section>
        </>
      )}

      <section className="panel" style={{ marginTop: 18 }}>
        <div className="ng-panel-head">
          <h3>실시간 매매 알림 · 일일 브리핑 연결</h3>
          <div className="ng-panel-head-btns">
            <button
              type="button"
              className="primary small"
              title="텔레그램·디스코드·슬랙 화면을 단계별로 보면서 1~2분 안에 따라 할 수 있는 도움말을 엽니다"
              onClick={() => setGuideModal({ channel: 'telegram', step: 0, view: 'guide' })}
            >
              📖 화면 보면서 따라 하기
            </button>
            <button
              type="button"
              className="small"
              title="연결 후 실제로 도착하는 매매 알림과 장 마감 일일 성적표 예시를 미리 봅니다"
              onClick={() => setGuideModal({ channel: 'telegram', step: 0, view: 'preview' })}
            >
              📊 알림 예시
            </button>
          </div>
        </div>
        <p className="muted small">
          화면을 보고 있지 않아도 자동매매 주문 접수·체결 완료·잔고 부족 스킵·비상 정지 알림과 장 마감 후 <b>오늘의 봇 성적표</b>를
          텔레그램·디스코드·슬랙으로 바로 받아볼 수 있습니다. 토큰과 웹훅 주소는 암호화되어 저장됩니다.
        </p>

        {/* Quick messenger guide cards */}
        <div className="ng-channel-strip">
          {NOTIFY_CHANNEL_LIST.map((ch) => {
            const isConn =
              ch.id === 'telegram'
                ? Boolean(notify?.telegram_configured)
                : ch.id === 'discord'
                  ? Boolean(notify?.discord_configured)
                  : Boolean(notify?.slack_configured)
            return (
              <div key={ch.id} className={`ng-channel-card ${isConn ? 'connected' : ''}`}>
                <div className="ng-channel-card-top">
                  <b>{ch.emoji} {ch.name}</b>
                  <span className={`status-badge ${isConn ? 'approved' : 'closed'}`}>
                    {isConn ? '연결됨 ✓' : '미연결'}
                  </span>
                </div>
                <div className="ng-channel-card-meta small muted">
                  <span>{ch.badge}</span>
                  <span>· {ch.duration}</span>
                </div>
                <p className="small muted ng-channel-card-desc">{ch.bestFor}</p>
                <button
                  type="button"
                  className="ng-card-btn"
                  title={`${ch.name} 연결 방법을 화면 하나하나 보면서 따라 합니다 (${ch.steps.length}단계)`}
                  onClick={() => setGuideModal({ channel: ch.id, step: 0, view: 'guide' })}
                >
                  화면별 도움말 ({ch.steps.length}단계) →
                </button>
              </div>
            )
          })}
        </div>

        {notify?.configured && (
          <dl className="kv" style={{ marginBottom: 12 }}>
            <dt>연결된 채널</dt>
            <dd>
              {[
                notify.telegram_configured && `텔레그램 (${notify.telegram_bot_token_masked})`,
                notify.discord_configured && `디스코드 (${notify.discord_webhook_masked})`,
                notify.slack_configured && `슬랙 (${notify.slack_webhook_masked})`,
              ].filter(Boolean).join(' · ')}
            </dd>
          </dl>
        )}
        <form
          className="form"
          autoComplete="off"
          onSubmit={async (e) => {
            e.preventDefault()
            setNBusy(true)
            setNMsg(null)
            try {
              const saved = await brokerApi.saveNotifications({
                enabled: nEnabled,
                paper_notify: nPaper,
                telegram_bot_token: tgToken.trim() ? tgToken.trim() : '__KEEP__',
                telegram_chat_id: tgChatId.trim(),
                discord_webhook_url: discordUrl.trim() ? discordUrl.trim() : '__KEEP__',
                slack_webhook_url: slackUrl.trim() ? slackUrl.trim() : '__KEEP__',
                events,
              })
              applyNotify(saved)
              setNMsg({ ok: true, text: '알림 설정이 저장되었습니다.' })
            } catch (err) {
              setNMsg({ ok: false, text: (err as Error).message })
            } finally {
              setNBusy(false)
            }
          }}
        >
          <label className="check" title="켜면 연결된 메신저로 자동매매 실시간 알림을 발송합니다">
            <input type="checkbox" checked={nEnabled} onChange={(e) => setNEnabled(e.target.checked)} />
            <span>실시간 메신저 알림 켜기</span>
          </label>
          <label className="check" title="기본은 실제 돈이 움직이는 실거래·반자동 알림만 보냅니다. 켜면 모의 모드 매매도 함께 알림을 받습니다">
            <input type="checkbox" checked={nPaper} onChange={(e) => setNPaper(e.target.checked)} />
            <span>모의(Paper) 모드 매매 내역도 알림 받기</span>
          </label>

          <div className="ng-field">
            <div className="ng-label-row">
              <label htmlFor="ng-tg-token" title="텔레그램 @BotFather에서 발급받은 봇 토큰을 입력하세요 (비워두면 기존 저장된 토큰을 유지합니다)">
                ✈️ 텔레그램 Bot Token {notify?.telegram_configured && <span className="muted small">· 등록됨 ({notify.telegram_bot_token_masked})</span>}
              </label>
              <button
                type="button"
                className="ng-help-link"
                title="텔레그램 @BotFather에서 봇 토큰을 발급받는 화면을 바로 봅니다"
                onClick={() => openFieldGuide('tg-token')}
              >
                ? 발급 화면 보기
              </button>
            </div>
            <input
              id="ng-tg-token"
              ref={tgTokenRef}
              className={focusHighlight === 'telegram' ? 'ng-input-pulse' : ''}
              type="password"
              autoComplete="new-password"
              value={tgToken}
              onChange={(e) => setTgToken(e.target.value)}
              placeholder={notify?.telegram_configured ? '변경할 때만 새 토큰 입력' : '123456789:ABCdef…'}
              title="텔레그램 @BotFather에서 발급받은 봇 토큰을 입력하세요"
              spellCheck={false}
            />
          </div>

          <div className="ng-field">
            <div className="ng-label-row">
              <label htmlFor="ng-tg-chatid" title="알림을 받을 내 텔레그램 대화방 ID(숫자)를 입력하세요">
                ✈️ 텔레그램 Chat ID
              </label>
              <button
                type="button"
                className="ng-help-link"
                title="내 텔레그램 숫자 Chat ID를 10초 만에 확인하는 화면을 바로 봅니다"
                onClick={() => openFieldGuide('tg-chat-id')}
              >
                ? Chat ID 찾는 법
              </button>
            </div>
            <input
              id="ng-tg-chatid"
              className={focusHighlight === 'telegram' ? 'ng-input-pulse' : ''}
              value={tgChatId}
              onChange={(e) => setTgChatId(e.target.value)}
              placeholder="예: 123456789"
              title="알림을 받을 내 텔레그램 Chat ID를 입력하세요"
              spellCheck={false}
            />
          </div>

          <div className="ng-field">
            <div className="ng-label-row">
              <label htmlFor="ng-dc-url" title="디스코드 채널 설정 → 연동 → 웹후크에서 복사한 https:// 주소를 붙여넣으세요">
                💬 디스코드 Webhook URL {notify?.discord_configured && <span className="muted small">· 등록됨 ({notify.discord_webhook_masked})</span>}
              </label>
              <button
                type="button"
                className="ng-help-link"
                title="디스코드 채널에서 웹후크 URL을 복사하는 화면을 바로 봅니다"
                onClick={() => openFieldGuide('discord-url')}
              >
                ? 웹후크 복사 방법
              </button>
            </div>
            <input
              id="ng-dc-url"
              ref={discordUrlRef}
              className={focusHighlight === 'discord' ? 'ng-input-pulse' : ''}
              type="password"
              autoComplete="new-password"
              value={discordUrl}
              onChange={(e) => setDiscordUrl(e.target.value)}
              placeholder={notify?.discord_configured ? '변경할 때만 새 URL 입력' : 'https://discord.com/api/webhooks/…'}
              title="디스코드 채널 웹훅 https:// 주소를 입력하세요"
              spellCheck={false}
            />
          </div>

          <div className="ng-field">
            <div className="ng-label-row">
              <label htmlFor="ng-sl-url" title="슬랙 Incoming Webhook에서 발급받은 https://hooks.slack.com/... 주소를 붙여넣으세요">
                💼 슬랙 Webhook URL {notify?.slack_configured && <span className="muted small">· 등록됨 ({notify.slack_webhook_masked})</span>}
              </label>
              <button
                type="button"
                className="ng-help-link"
                title="슬랙 Incoming Webhook URL을 발급받는 화면을 바로 봅니다"
                onClick={() => openFieldGuide('slack-url')}
              >
                ? 웹훅 발급 화면
              </button>
            </div>
            <input
              id="ng-sl-url"
              ref={slackUrlRef}
              className={focusHighlight === 'slack' ? 'ng-input-pulse' : ''}
              type="password"
              autoComplete="new-password"
              value={slackUrl}
              onChange={(e) => setSlackUrl(e.target.value)}
              placeholder={notify?.slack_configured ? '변경할 때만 새 URL 입력' : 'https://hooks.slack.com/services/…'}
              title="슬랙 Incoming Webhook https:// 주소를 입력하세요"
              spellCheck={false}
            />
          </div>

          <fieldset className="costs">
            <legend>수신할 알림 종류</legend>
            {([
              ['order', '매수·매도 주문 전송 및 반자동 승인 대기 알림'],
              ['fill', '체결 완료 및 리밸런싱 완료 알림'],
              ['error', '잔고 부족 스킵·주문 거부·API 오류 알림'],
              ['halt', '비상 전체 중지 및 서킷브레이커 보호 잠금 긴급 알림'],
              ['briefing', '국내·미국 정규장 마감 직후 일일 성적표 브리핑'],
            ] as [keyof NotificationEvents, string][]).map(([k, label]) => (
              <label key={k} className="check small" title={`${label} 수신 여부를 켜거나 끕니다`}>
                <input
                  type="checkbox"
                  checked={events[k]}
                  onChange={(e) => setEvents({ ...events, [k]: e.target.checked })}
                />
                <span>{label}</span>
              </label>
            ))}
          </fieldset>

          <div className="modal-actions" style={{ justifyContent: 'flex-start' }}>
            <button type="submit" className="primary" disabled={nBusy} title="입력한 알림 채널과 수신 옵션을 암호화해 저장합니다">
              {nBusy ? '저장 중…' : '알림 설정 저장'}
            </button>
            {notify?.configured && (
              <>
                <button
                  type="button"
                  disabled={nBusy}
                  title="지금 바로 내 텔레그램·디스코드·슬랙으로 테스트 메시지가 잘 도착하는지 확인합니다"
                  onClick={async () => {
                    setNBusy(true)
                    setNMsg(null)
                    try {
                      const r = await brokerApi.testNotifications()
                      setNMsg({ ok: true, text: `테스트 알림 발송 성공 (${r.sent.join(', ')})` })
                    } catch (err) {
                      setNMsg({ ok: false, text: (err as Error).message })
                    } finally {
                      setNBusy(false)
                    }
                  }}
                >
                  테스트 알림 보내기
                </button>
                <button
                  type="button"
                  className="danger"
                  disabled={nBusy}
                  title="등록된 모든 메신저 토큰과 웹훅 주소를 삭제합니다"
                  onClick={async () => {
                    if (!confirm('등록된 알림 채널 정보를 모두 삭제할까요?')) return
                    applyNotify(await brokerApi.deleteNotifications())
                    setNMsg({ ok: true, text: '알림 채널 정보가 삭제되었습니다.' })
                  }}
                >
                  알림 삭제
                </button>
              </>
            )}
          </div>
        </form>
        {nMsg && <p className={nMsg.ok ? 'ok' : 'error'}>{nMsg.text}</p>}
      </section>

      <section className="panel" style={{ marginTop: 18 }}>
        <h3>앱 버전 · 시스템 정보</h3>
        <dl className="kv" style={{ marginBottom: 0 }}>
          <dt>웹 화면 버전</dt>
          <dd><code>v{APP_VERSION}</code></dd>
          <dt>서버 API 버전</dt>
          <dd>
            <code>{serverVer ? `v${serverVer}` : '확인 중…'}</code>
            {serverVer && isNewerVersion(serverVer, APP_VERSION) && (
              <>
                <span className="warn small" style={{ marginLeft: 8 }}>
                  새 버전이 배포되었습니다.
                </span>{' '}
                <button
                  type="button"
                  className="primary small"
                  style={{ marginLeft: 6 }}
                  title="이전 캐시를 비우고 최신 버전 웹 화면을 즉시 다시 불러옵니다"
                  onClick={() => void forceReloadApp(serverVer)}
                >
                  지금 새로고침
                </button>
              </>
            )}
          </dd>
        </dl>
      </section>

      {tossGuideModal && (
        <TossGuideModal
          initialStep={tossGuideModal.step}
          initialView={tossGuideModal.view}
          configured={Boolean(status?.configured)}
          serverIp={serverIp}
          onClose={() => setTossGuideModal(null)}
          onFocusField={focusTossInput}
        />
      )}

      {kiwoomGuideModal && (
        <KiwoomGuideModal
          initialStep={kiwoomGuideModal.step}
          initialView={kiwoomGuideModal.view}
          configured={Boolean(kwStatus?.configured)}
          serverIp={serverIp}
          onClose={() => setKiwoomGuideModal(null)}
          onFocusField={focusKiwoomInput}
        />
      )}

      {guideModal && (
        <NotificationGuideModal
          initialChannel={guideModal.channel}
          initialStep={guideModal.step}
          initialView={guideModal.view}
          configured={{
            telegram: Boolean(notify?.telegram_configured),
            discord: Boolean(notify?.discord_configured),
            slack: Boolean(notify?.slack_configured),
          }}
          onClose={() => setGuideModal(null)}
          onFocusField={focusChannelInput}
        />
      )}
    </div>
  )
}
