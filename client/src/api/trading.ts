import { api } from './client'

// TODO: typed Order / Position once the broker integration lands.
export const tradingApi = {
  orders: () => api<{ items: unknown[] }>('/trading/orders'),
  placeOrder: (order: unknown) => api('/trading/orders', { method: 'POST', json: order }),
  strategies: () => api<{ items: unknown[] }>('/trading/strategies'),
}
