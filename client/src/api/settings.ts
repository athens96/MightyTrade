import { api } from './client'

export type Settings = Record<string, unknown>

export const settingsApi = {
  get: () => api<{ settings: Settings }>('/users/me/settings'),
  put: (settings: Settings) => api<{ settings: Settings }>('/users/me/settings', { method: 'PUT', json: { settings } }),
}
