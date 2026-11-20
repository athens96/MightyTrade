import { useState } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { GoogleLogin } from '@react-oauth/google'
import { useAuth } from '../auth/AuthContext'

export default function LoginPage() {
  const { status, loginWithGoogle } = useAuth()
  const navigate = useNavigate()
  const [error, setError] = useState('')

  if (status === 'authenticated') return <Navigate to="/" replace />

  return (
    <div className="center card">
      <h1>MightyTrade</h1>
      <p className="muted">Google 계정으로 로그인 후 OTP 인증이 필요합니다.</p>
      <GoogleLogin
        onSuccess={async ({ credential }) => {
          if (!credential) return setError('Google credential이 없습니다')
          try {
            await loginWithGoogle(credential)
            navigate('/otp')
          } catch (e) {
            setError((e as Error).message)
          }
        }}
        onError={() => setError('Google 로그인 실패')}
      />
      {error && <p className="error">{error}</p>}
    </div>
  )
}
