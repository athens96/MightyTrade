import { api } from '../api/client'

declare const __APP_VERSION__: string | undefined

export const APP_VERSION: string = typeof __APP_VERSION__ === 'string' && __APP_VERSION__ ? __APP_VERSION__ : '0.1.0'

export interface HealthInfo {
  status: string
  version?: string
}

export const fetchHealth = () => api<HealthInfo>('/health')
