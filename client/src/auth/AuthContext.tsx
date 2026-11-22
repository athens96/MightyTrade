import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { authApi, type NextStep } from '../api/auth'
import { onSessionExpired } from '../api/client'
import type { User } from '../api/types'

// loading -> anonymous -> (Google) pending -> (OTP) authenticated
type Status = 'loading' | 'anonymous' | 'pending' | 'authenticated'

const RETURN_TO_KEY = 'mt.auth.returnTo'
const RECHECK_MIN_MS = 15_000

export function saveReturnTo(path: string) {
  if (!path || path.startsWith('/login') || path.startsWith('/otp')) return
  try {
    sessionStorage.setItem(RETURN_TO_KEY, path)
  } catch {
    // ignore storage quota / private mode errors
  }
}

export function consumeReturnTo(): string | null {
  try {
    const v = sessionStorage.getItem(RETURN_TO_KEY)
    sessionStorage.removeItem(RETURN_TO_KEY)
    return v && !v.startsWith('/login') && !v.startsWith('/otp') ? v : null
  } catch {
    return null
  }
}

interface AuthState {
  status: Status
  user: User | null
  nextStep: NextStep | null
  sessionExpired: boolean
  clearSessionExpired: () => void
  loginWithGoogle: (credential: string) => Promise<NextStep>
  verifyOtp: (code: string) => Promise<void>
  logout: () => Promise<void>
}

const AuthContext = createContext<AuthState | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status>('loading')
  const [user, setUser] = useState<User | null>(null)
  const [nextStep, setNextStep] = useState<NextStep | null>(null)
  const [sessionExpired, setSessionExpired] = useState(false)
  const statusRef = useRef<Status>('loading')
  const lastCheckAt = useRef<number>(Date.now())

  const updateStatus = useCallback((next: Status) => {
    statusRef.current = next
    setStatus(next)
  }, [])

  // 1) Initial session bootstrap on page load (anonymous visitors should NOT see an "expired" banner)
  useEffect(() => {
    authApi
      .me()
      .then(({ user }) => {
        lastCheckAt.current = Date.now()
        setUser(user)
        setSessionExpired(false)
        updateStatus('authenticated')
      })
      .catch(() => updateStatus('anonymous'))
  }, [updateStatus])

  // 2) Global 401 session-expired interceptor from any API request across the app
  useEffect(() => {
    return onSessionExpired(() => {
      const cur = statusRef.current
      if (cur === 'authenticated' || cur === 'pending') {
        setUser(null)
        setNextStep(null)
        setSessionExpired(true)
        updateStatus('anonymous')
      }
    })
  }, [updateStatus])

  // 3) Proactive re-check when returning to a long-idle background tab / window
  useEffect(() => {
    const checkOnWake = () => {
      if (document.visibilityState !== 'visible') return
      if (statusRef.current !== 'authenticated') return
      const now = Date.now()
      if (now - lastCheckAt.current < RECHECK_MIN_MS) return
      lastCheckAt.current = now
      authApi
        .me()
        .then(({ user }) => setUser(user))
        .catch(() => undefined) // 401 triggers `onSessionExpired` above automatically
    }
    document.addEventListener('visibilitychange', checkOnWake)
    window.addEventListener('focus', checkOnWake)
    return () => {
      document.removeEventListener('visibilitychange', checkOnWake)
      window.removeEventListener('focus', checkOnWake)
    }
  }, [])

  const clearSessionExpired = useCallback(() => setSessionExpired(false), [])

  const loginWithGoogle = useCallback(async (credential: string) => {
    setSessionExpired(false)
    const res = await authApi.google(credential)
    if (res.next === 'approval_pending' || res.next === 'rejected') return res.next // no session until approved
    setUser(res.user)
    setNextStep(res.next)
    updateStatus('pending')
    return res.next
  }, [updateStatus])

  const verifyOtp = useCallback(async (code: string) => {
    const res = await authApi.otpVerify(code)
    lastCheckAt.current = Date.now()
    setUser(res.user)
    setNextStep(null)
    setSessionExpired(false)
    updateStatus('authenticated')
  }, [updateStatus])

  const logout = useCallback(async () => {
    // Mark anonymous intentionally first so `/auth/logout` or in-flight polls don't show an expired banner
    updateStatus('anonymous')
    setUser(null)
    setNextStep(null)
    setSessionExpired(false)
    consumeReturnTo()
    await authApi.logout().catch(() => undefined)
  }, [updateStatus])

  return (
    <AuthContext.Provider value={{ status, user, nextStep, sessionExpired, clearSessionExpired, loginWithGoogle, verifyOtp, logout }}>
      {children}
    </AuthContext.Provider>
  )
}

export function useAuth() {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth must be used inside AuthProvider')
  return ctx
}
