import { api } from './client'
import { sealWithKey, type TransportKey } from '../lib/secureEnvelope'

export interface TossCredentialStatus {
  configured: boolean
  client_id_masked?: string
  account_seq?: number | null
  verified_at?: string
  server_ip?: string | null
}

export interface NotificationEvents {
  order: boolean
  fill: boolean
  error: boolean
  halt: boolean
  briefing: boolean
}

export interface NotificationSettingsView {
  configured: boolean
  enabled: boolean
  paper_notify: boolean
  telegram_configured: boolean
  telegram_bot_token_masked: string | null
  telegram_chat_id: string
  discord_configured: boolean
  discord_webhook_masked: string | null
  slack_configured: boolean
  slack_webhook_masked: string | null
  events: NotificationEvents
  updated_at: string | null
}

export interface NotificationSaveInput {
  enabled: boolean
  paper_notify: boolean
  telegram_bot_token?: string
  telegram_chat_id?: string
  discord_webhook_url?: string
  slack_webhook_url?: string
  events: NotificationEvents
}

export const brokerApi = {
  tossStatus: () => api<TossCredentialStatus>('/users/me/broker/toss'),

  /** Secrets are encrypted in the browser; the request body never contains them in plaintext. */
  saveToss: async (userId: string, input: { client_id: string; client_secret: string; otp_code: string }) => {
    const key = await api<TransportKey>('/users/transport-key')
    const envelope = await sealWithKey(key, input, userId)
    return api<TossCredentialStatus>('/users/me/broker/toss', { method: 'PUT', json: { envelope } })
  },

  deleteToss: () => api<TossCredentialStatus>('/users/me/broker/toss', { method: 'DELETE' }),

  notificationsStatus: () => api<NotificationSettingsView>('/users/me/notifications'),
  saveNotifications: (input: NotificationSaveInput) =>
    api<NotificationSettingsView>('/users/me/notifications', { method: 'PUT', json: input }),
  deleteNotifications: () => api<NotificationSettingsView>('/users/me/notifications', { method: 'DELETE' }),
  testNotifications: () => api<{ sent: string[]; errors: Record<string, string> }>('/users/me/notifications/test', { method: 'POST' }),
}
