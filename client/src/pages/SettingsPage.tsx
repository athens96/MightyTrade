import { useEffect, useState, type FormEvent } from 'react'
import { brokerApi, type NotificationEvents, type NotificationSettingsView, type TossCredentialStatus } from '../api/broker'
import { useAuth } from '../auth/AuthContext'
import { FeatureGuideBox } from '../components/GuideBox'
import { FEATURE_GUIDES } from '../lib/featureGuides'
import { APP_VERSION, fetchHealth } from '../lib/version'

const DEFAULT_EVENTS: NotificationEvents = {
  order: true,
  fill: true,
  error: true,
  halt: true,
  briefing: true,
}

export default function SettingsPage() {
  const { user } = useAuth()
  const [status, setStatus] = useState<TossCredentialStatus | null>(null)
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [otpCode, setOtpCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null)

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
    brokerApi.tossStatus().then(setStatus).catch((e) => setMessage({ ok: false, text: e.message }))
    brokerApi.notificationsStatus().then(applyNotify).catch(() => undefined)
    fetchHealth().then((h) => setServerVer(h.version ?? null)).catch(() => undefined)
  }, [])

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
      setMessage({ ok: true, text: '삭제되었습니다.' })
    } catch (err) {
      setMessage({ ok: false, text: (err as Error).message })
    }
  }

  return (
    <div className="settings">
      <h2>토스증권 API 연결</h2>
      <FeatureGuideBox guide={FEATURE_GUIDES.settings} />

      <section className="panel">
        <h3>현재 상태</h3>
        {!status && <p className="muted">불러오는 중…</p>}
        {status && !status.configured && <p className="muted">등록된 키가 없습니다.</p>}
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
            <button className="danger" title="서버에 암호화되어 저장된 내 토스증권 API 키를 즉시 삭제합니다" onClick={remove}>키 삭제</button>
          </>
        )}
      </section>

      <section className="panel">
        <h3>{status?.configured ? '키 변경' : '키 등록'}</h3>
        <ol className="muted small">
          <li>토스증권 PC 웹(WTS) → 설정 → Open API에서 client_id / client_secret 발급</li>
          <li>같은 화면의 <b>허용 IP 관리</b>에 이 서비스 서버의 IP 등록</li>
          <li>아래에 입력 후 OTP 코드로 확인 (키는 브라우저에서 암호화되어 전송되고, 서버에도 암호화되어 저장됩니다)</li>
        </ol>
        <form className="form" onSubmit={submit} autoComplete="off">
          <label title="토스증권 PC 웹(WTS) → 설정 → Open API에서 발급받은 Client ID를 붙여넣으세요">
            Client ID
            <input
              value={clientId}
              onChange={(e) => setClientId(e.target.value)}
              placeholder="tsck_live_…"
              title="토스증권 Open API에서 발급받은 Client ID(tsck_live_... 등)를 붙여넣으세요"
              spellCheck={false}
            />
          </label>
          <label title="토스증권 Open API에서 함께 발급받은 비밀 키(Client Secret)를 입력하세요 (암호화되어 전송·저장됩니다)">
            Client Secret
            <input
              type="password"
              autoComplete="new-password"
              value={clientSecret}
              onChange={(e) => setClientSecret(e.target.value)}
              title="토스증권 Open API에서 발급받은 Client Secret을 입력하세요 (암호화되어 전송·저장됩니다)"
              spellCheck={false}
            />
          </label>
          <label title="본인 확인을 위해 스마트폰 인증 앱(Google Authenticator 등)에 표시된 6자리 숫자를 입력하세요">
            OTP 코드
            <input
              inputMode="numeric"
              autoComplete="one-time-code"
              maxLength={6}
              value={otpCode}
              onChange={(e) => setOtpCode(e.target.value.replace(/\D/g, ''))}
              placeholder="6자리"
              title="스마트폰 인증 앱에 표시된 6자리 일회용 숫자를 입력하세요"
            />
          </label>
          <button
            type="submit"
            disabled={busy || !clientId.trim() || !clientSecret.trim() || otpCode.length !== 6}
            title="입력한 API 키가 정상 작동하는지 토스증권에 먼저 확인한 뒤 암호화해 안전하게 저장합니다"
          >
            {busy ? '토스증권에 확인 중…' : '확인 후 저장'}
          </button>
        </form>
        {message && <p className={message.ok ? 'ok' : 'error'}>{message.text}</p>}
      </section>

      <section className="panel" style={{ marginTop: 18 }}>
        <h3>실시간 매매 알림 · 일일 브리핑 연결</h3>
        <p className="muted small">
          화면을 보고 있지 않아도 자동매매 주문 접수·체결 완료·잔고 부족 스킵·비상 정지 알림과 장 마감 후 <b>오늘의 봇 성적표</b>를
          텔레그램·디스코드·슬랙으로 바로 받아볼 수 있습니다. 토큰과 웹훅 주소는 암호화되어 저장됩니다.
        </p>
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

          <label title="텔레그램 @BotFather에서 발급받은 봇 토큰을 입력하세요 (비워두면 기존 저장된 토큰을 유지합니다)">
            텔레그램 Bot Token {notify?.telegram_configured && <span className="muted small">· 등록됨 ({notify.telegram_bot_token_masked})</span>}
            <input
              type="password"
              autoComplete="new-password"
              value={tgToken}
              onChange={(e) => setTgToken(e.target.value)}
              placeholder={notify?.telegram_configured ? '변경할 때만 새 토큰 입력' : '123456789:ABCdef…'}
              title="텔레그램 @BotFather에서 발급받은 봇 토큰을 입력하세요"
              spellCheck={false}
            />
          </label>
          <label title="알림을 받을 내 텔레그램 대화방 ID(숫자)를 입력하세요">
            텔레그램 Chat ID
            <input
              value={tgChatId}
              onChange={(e) => setTgChatId(e.target.value)}
              placeholder="예: 123456789"
              title="알림을 받을 내 텔레그램 Chat ID를 입력하세요"
              spellCheck={false}
            />
          </label>
          <label title="디스코드 채널 설정 → 연동 → 웹후크에서 복사한 https:// 주소를 붙여넣으세요">
            디스코드 Webhook URL {notify?.discord_configured && <span className="muted small">· 등록됨 ({notify.discord_webhook_masked})</span>}
            <input
              type="password"
              autoComplete="new-password"
              value={discordUrl}
              onChange={(e) => setDiscordUrl(e.target.value)}
              placeholder={notify?.discord_configured ? '변경할 때만 새 URL 입력' : 'https://discord.com/api/webhooks/…'}
              title="디스코드 채널 웹훅 https:// 주소를 입력하세요"
              spellCheck={false}
            />
          </label>
          <label title="슬랙 Incoming Webhook에서 발급받은 https://hooks.slack.com/... 주소를 붙여넣으세요">
            슬랙 Webhook URL {notify?.slack_configured && <span className="muted small">· 등록됨 ({notify.slack_webhook_masked})</span>}
            <input
              type="password"
              autoComplete="new-password"
              value={slackUrl}
              onChange={(e) => setSlackUrl(e.target.value)}
              placeholder={notify?.slack_configured ? '변경할 때만 새 URL 입력' : 'https://hooks.slack.com/services/…'}
              title="슬랙 Incoming Webhook https:// 주소를 입력하세요"
              spellCheck={false}
            />
          </label>

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
            {serverVer && serverVer !== APP_VERSION && (
              <span className="warn small" style={{ marginLeft: 8 }}>
                새 버전이 배포되었습니다. 브라우저 새로고침(⌘R / F5)으로 최신 화면을 불러오세요.
              </span>
            )}
          </dd>
        </dl>
      </section>
    </div>
  )
}
