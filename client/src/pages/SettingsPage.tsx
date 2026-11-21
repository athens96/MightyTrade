import { useEffect, useState, type FormEvent } from 'react'
import { brokerApi, type TossCredentialStatus } from '../api/broker'
import { useAuth } from '../auth/AuthContext'
import { FeatureGuideBox } from '../components/GuideBox'
import { FEATURE_GUIDES } from '../lib/featureGuides'

export default function SettingsPage() {
  const { user } = useAuth()
  const [status, setStatus] = useState<TossCredentialStatus | null>(null)
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [otpCode, setOtpCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null)

  useEffect(() => {
    brokerApi.tossStatus().then(setStatus).catch((e) => setMessage({ ok: false, text: e.message }))
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
    </div>
  )
}
