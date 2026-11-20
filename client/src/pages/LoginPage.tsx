import { useState } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { GoogleLogin } from '@react-oauth/google'
import { useAuth } from '../auth/AuthContext'

export default function LoginPage() {
  const { status, loginWithGoogle } = useAuth()
  const navigate = useNavigate()
  const [error, setError] = useState('')
  const [gate, setGate] = useState<'approval_pending' | 'rejected' | null>(null)

  if (status === 'authenticated') return <Navigate to="/" replace />

  return (
    <div className="center card">
      <h1>Mighty Trade</h1>
      <p className="muted">Google 계정으로 로그인하세요. 처음이면 관리자 승인 후 이용할 수 있습니다.</p>
      <GoogleLogin
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
    </div>
  )
}
