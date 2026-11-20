import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react'
import { authApi, type NextStep } from '../api/auth'
import type { User } from '../api/types'

// loading -> anonymous -> (Google) pending -> (OTP) authenticated
type Status = 'loading' | 'anonymous' | 'pending' | 'authenticated'

interface AuthState {
  status: Status
  user: User | null
  nextStep: NextStep | null
  loginWithGoogle: (credential: string) => Promise<NextStep>
  verifyOtp: (code: string) => Promise<void>
  logout: () => Promise<void>
}

const AuthContext = createContext<AuthState | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status>('loading')
  const [user, setUser] = useState<User | null>(null)
  const [nextStep, setNextStep] = useState<NextStep | null>(null)

  useEffect(() => {
    authApi
      .me()
      .then(({ user }) => {
        setUser(user)
        setStatus('authenticated')
      })
      .catch(() => setStatus('anonymous'))
  }, [])

  const loginWithGoogle = useCallback(async (credential: string) => {
    const res = await authApi.google(credential)
    if (res.next === 'approval_pending' || res.next === 'rejected') return res.next // no session until approved
    setUser(res.user)
    setNextStep(res.next)
    setStatus('pending')
    return res.next
  }, [])

  const verifyOtp = useCallback(async (code: string) => {
    const res = await authApi.otpVerify(code)
    setUser(res.user)
    setNextStep(null)
    setStatus('authenticated')
  }, [])

  const logout = useCallback(async () => {
    await authApi.logout().catch(() => undefined)
    setUser(null)
    setNextStep(null)
    setStatus('anonymous')
  }, [])

  return (
    <AuthContext.Provider value={{ status, user, nextStep, loginWithGoogle, verifyOtp, logout }}>
      {children}
    </AuthContext.Provider>
  )
}

export function useAuth() {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be used inside AuthProvider')
  return ctx
}
