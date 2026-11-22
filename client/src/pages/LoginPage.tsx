import { useState } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { GoogleLogin } from '@react-oauth/google'
import { useAuth } from '../auth/AuthContext'
import AuthShell from '../components/auth/AuthShell'
import { APP_VERSION } from '../lib/version'

// Google's button takes a fixed pixel width: fill the card (card 420px - padding), narrower on phones.
const buttonWidth = () => Math.min(356, window.innerWidth - 84)

export default function LoginPage() {
  const { status, sessionExpired, loginWithGoogle } = useAuth()
  const navigate = useNavigate()
  const [error, setError] = useState('')
  const [gate, setGate] = useState<'approval_pending' | 'rejected' | null>(null)

  if (status === 'authenticated') return <Navigate to="/" replace />

  return (
    <AuthShell>
      <img className="auth-mascot" src="/mascot-256.png" alt="" width={112} height={112} />
      <h1 className="auth-title">Mighty <span>Trade</span></h1>
      <p className="auth-tag">시장을 읽고, 전략을 검증하고, 자동으로 실행합니다.</p>
      {sessionExpired && (
        <p className="warn small" style={{ width: '100%', marginBottom: 14, textAlign: 'center' }}>
          로그인 유효시간이 만료되어 안전을 위해 로그인 화면으로 이동했습니다. 다시 로그인해 주세요.
        </p>
      )}
      <div className="auth-google">
        <GoogleLogin
          theme="outline"
          size="large"
          text="signin_with"
          shape="rectangular"
          width={buttonWidth()}
          onSuccess={async ({ credential }) => {
            if (!credential) return setError('Google credential이 없습니다')
            try {
              const next = await loginWithGoogle(credential)
              if (next === 'approval_pending' || next === 'rejected') setGate(next)
              else navigate('/otp')
            } catch (e) {
              setError((e as Error).message)
            }
          }}
          onError={() => setError('Google 로그인 실패')}
        />
      </div>
      {gate === 'approval_pending' && (
        <div className="gate">
          <b>가입 신청이 접수되었습니다.</b>
          <p className="muted small">관리자가 승인하면 다시 로그인해 OTP를 등록한 뒤 이용할 수 있습니다.</p>
        </div>
      )}
      {gate === 'rejected' && (
        <div className="gate rejected">
          <b>가입이 승인되지 않았습니다.</b>
          <p className="muted small">문의가 필요하면 관리자에게 연락하세요.</p>
        </div>
      )}
      {error && <p className="error">{error}</p>}
      <ul className="auth-secure">
        <li>OTP 2단계 인증</li>
        <li>API 키 암호화 저장</li>
        <li>관리자 승인제</li>
      </ul>
      <p className="auth-note">처음 로그인하면 관리자 승인 후 이용할 수 있습니다 · <span title="현재 배포된 마이티 트레이드 버전">v{APP_VERSION}</span></p>
    </AuthShell>
  )
}
