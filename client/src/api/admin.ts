import { api } from './client'
import type { User } from './types'

export interface AdminUser extends User {
  created_at: string | null
  last_login_at: string | null
  approved_at: string | null
  approved_by: string | null
  has_toss_key: boolean
  bots: number
  live_bots: number
}

export type UserFilter = 'pending' | 'approved' | 'rejected' | 'all'

export const adminApi = {
  summary: () => api<{ pending: number }>('/admin/summary'),
  users: (status: UserFilter) =>
    api<{ items: AdminUser[]; counts: Record<UserFilter, number> }>(`/admin/users?status=${status}`),
  approve: (id: string) => api<{ user: AdminUser }>(`/admin/users/${id}/approve`, { method: 'POST' }),
  reject: (id: string) => api<{ user: AdminUser }>(`/admin/users/${id}/reject`, { method: 'POST' }),
  remove: (id: string, otp_code: string, force = false) =>
    api<{ ok: true; removed: Record<string, number> }>(`/admin/users/${id}`, { method: 'DELETE', json: { otp_code, force } }),
}
