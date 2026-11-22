import { api } from './client'
import type { RequestStatus, UserRequestItem } from './requests'
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

export interface AdminRequestItem extends UserRequestItem {
  user_id: string | null
  user_email: string | null
  user_name: string | null
  replied_by: string | null
}

export type UserFilter = 'pending' | 'approved' | 'rejected' | 'all'
export type RequestFilter = RequestStatus | 'all'

export const adminApi = {
  summary: () => api<{ pending: number; open_requests: number }>('/admin/summary'),
  users: (status: UserFilter) =>
    api<{ items: AdminUser[]; counts: Record<UserFilter, number> }>(`/admin/users?status=${status}`),
  approve: (id: string) => api<{ user: AdminUser }>(`/admin/users/${id}/approve`, { method: 'POST' }),
  reject: (id: string) => api<{ user: AdminUser }>(`/admin/users/${id}/reject`, { method: 'POST' }),
  remove: (id: string, otp_code: string, force = false) =>
    api<{ ok: true; removed: Record<string, number> }>(`/admin/users/${id}`, { method: 'DELETE', json: { otp_code, force } }),
  requests: (status: RequestFilter) =>
    api<{ items: AdminRequestItem[]; counts: Record<RequestFilter, number> }>(`/admin/requests?status=${status}`),
  updateRequest: (id: string, input: { status?: RequestStatus; admin_reply?: string }) =>
    api<{ item: AdminRequestItem }>(`/admin/requests/${id}`, { method: 'PATCH', json: input }),
  deleteRequest: (id: string) =>
    api<{ ok: true }>(`/admin/requests/${id}`, { method: 'DELETE' }),
}
