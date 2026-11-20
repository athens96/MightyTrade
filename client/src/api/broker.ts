import { api } from './client'
import { sealWithKey, type TransportKey } from '../lib/secureEnvelope'

export interface TossCredentialStatus {
  configured: boolean
  client_id_masked?: string
  account_seq?: number | null
  verified_at?: string
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
}
