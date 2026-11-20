import { useEffect, useRef, useState, type FormEvent } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { QRCodeSVG } from 'qrcode.react'
import { authApi } from '../api/auth'
import { useAuth } from '../auth/AuthContext'

export default function OtpPage() {
  const { status, nextStep, verifyOtp } = useAuth()
  const navigate = useNavigate()
  const [setup, setSetup] = useState<{ otpauth_url: string; secret: string } | null>(null)
  const [code, setCode] = useState('')
  const [error, setError] = useState('')
  const requested = useRef(false)

  useEffect(() => {
    // Guard against StrictMode double-invoke: a second call would rotate the secret.
    if (nextStep !== 'otp_setup' || requested.current) return
    requested.current = true
    authApi.otpSetup().then(setSetup).catch((e) => setError(e.message))
  }, [nextStep])

  if (status === 'authenticated') return <Navigate to="/" replace />
  if (status !== 'pending' && status !== 'loading') return <Navigate to="/login" replace />

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setError('')
    try {
      await verifyOtp(code)
      navigate('/')
    } catch (err) {
      setError((err as Error).message)
    }
  }

  return (
    <div className="center card">
      <h2>{nextStep === 'otp_setup' ? 'OTP 등록' : 'OTP 인증'}</h2>
      {nextStep === 'otp_setup' && setup && (
        <>
          <p className="muted">Google Authenticator 등 OTP 앱으로 QR을 스캔하세요.</p>
          <div className="qr"><QRCodeSVG value={setup.otpauth_url} size={180} /></div>
          <code className="muted">{setup.secret}</code>
        </>
      )}
      <form onSubmit={submit}>
        <input
          inputMode="numeric"
          autoComplete="one-time-code"
          maxLength={6}
          placeholder="6자리 코드"
          value={code}
          onChange={(e) => setCode(e.target.value.replace(/\D/g, ''))}
          autoFocus
        />
        <button type="submit" disabled={code.length !== 6}>확인</button>
      </form>
      {error && <p className="error">{error}</p>}
    </div>
  )
}
