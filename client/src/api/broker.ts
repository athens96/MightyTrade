import { api } from './client'
import { sealWithKey, type TransportKey } from '../lib/secureEnvelope'

export type BrokerKind = 'toss' | 'kiwoom'

export interface TossCredentialStatus {
  configured: boolean
  client_id_masked?: string
  account_seq?: number | null
  verified_at?: string
  server_ip?: string | null
}

export interface KiwoomCredentialStatus {
  configured: boolean
  appkey_masked?: string | null
  account_no_masked?: string | null
  is_mock?: boolean
  verified_at?: string | null
  server_ip?: string | null
}

export interface BrokersOverview {
  active_broker: BrokerKind
  server_ip?: string | null
  toss: TossCredentialStatus
  kiwoom: KiwoomCredentialStatus
}

export interface KiwoomConditionItem {
  seq: string
  name: string
}

export interface KiwoomConditionMatch {
  symbol: string
  name: string
  price: number
  change: number
  change_pct: number
  volume: number
}

export interface KiwoomConditionMatchesResponse {
  seq: string
  market: 'KR'
  is_mock: boolean
  count: number
  items: KiwoomConditionMatch[]
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
  brokersOverview: () => api<BrokersOverview>('/users/me/brokers'),
  setActiveBroker: (broker: BrokerKind) =>
    api<BrokersOverview>('/users/me/broker/active', { method: 'PUT', json: { broker } }),

  tossStatus: () => api<TossCredentialStatus>('/users/me/broker/toss'),

  /** Secrets are encrypted in the browser; the request body never contains them in plaintext. */
  saveToss: async (userId: string, input: { client_id: string; client_secret: string; otp_code: string }) => {
    const key = await api<TransportKey>('/users/transport-key')
    const envelope = await sealWithKey(key, input, userId)
    return api<TossCredentialStatus>('/users/me/broker/toss', { method: 'PUT', json: { envelope } })
  },

  deleteToss: () => api<TossCredentialStatus>('/users/me/broker/toss', { method: 'DELETE' }),

  kiwoomStatus: () => api<KiwoomCredentialStatus>('/users/me/broker/kiwoom'),

  saveKiwoom: async (
    userId: string,
    input: {
      appkey: string
      secretkey: string
      account_no?: string
      is_mock: boolean
      otp_code: string
      activate?: boolean
    },
  ) => {
    const key = await api<TransportKey>('/users/transport-key')
    const envelope = await sealWithKey(key, input, userId)
    return api<KiwoomCredentialStatus>('/users/me/broker/kiwoom', { method: 'PUT', json: { envelope } })
  },

  deleteKiwoom: () => api<KiwoomCredentialStatus>('/users/me/broker/kiwoom', { method: 'DELETE' }),

  kiwoomConditions: () =>
    api<{ is_mock: boolean; count: number; items: KiwoomConditionItem[] }>('/kiwoom/conditions'),

  kiwoomConditionMatches: (seq: string) =>
    api<KiwoomConditionMatchesResponse>(`/kiwoom/conditions/${encodeURIComponent(seq)}/matches`),

  notificationsStatus: () => api<NotificationSettingsView>('/users/me/notifications'),
  saveNotifications: (input: NotificationSaveInput) =>
    api<NotificationSettingsView>('/users/me/notifications', { method: 'PUT', json: input }),
  deleteNotifications: () => api<NotificationSettingsView>('/users/me/notifications', { method: 'DELETE' }),
  testNotifications: () => api<{ sent: string[]; errors: Record<string, string> }>('/users/me/notifications/test', { method: 'POST' }),
}
