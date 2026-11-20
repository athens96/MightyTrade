import { api } from './client'
import type { User } from './types'

export type NextStep = 'otp_setup' | 'otp_verify'

export const authApi = {
  google: (credential: string) =>
    api<{ next: NextStep; user: User }>('/auth/google', { method: 'POST', json: { credential } }),
  otpSetup: () => api<{ otpauth_url: string; secret: string }>('/auth/otp/setup', { method: 'POST' }),
  otpVerify: (code: string) => api<{ user: User }>('/auth/otp/verify', { method: 'POST', json: { code } }),
  me: () => api<{ user: User }>('/auth/me'),
  logout: () => api<{ ok: true }>('/auth/logout', { method: 'POST' }),
}
