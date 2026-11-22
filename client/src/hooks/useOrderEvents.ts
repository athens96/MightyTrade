import { useEffect, useState } from 'react'
import { authApi } from '../api/auth'

export interface Toast {
  id: number
  text: string
  tone: 'ok' | 'warn' | 'error'
}

const EVENT_TEXT: Record<string, [string, Toast['tone']]> = {
  PENDING: ['접수', 'ok'], PARTIAL_FILL: ['부분 체결', 'ok'], FILL: ['체결', 'ok'], CANCELED: ['취소됨', 'warn'],
  REPLACED: ['정정됨', 'ok'], REJECTED: ['거부됨', 'error'], CANCEL_REJECTED: ['취소 거부', 'error'], REPLACE_REJECTED: ['정정 거부', 'error'],
}

/** Live order events for the logged-in account (server SSE relay of Toss personal:order). */
export function useOrderEvents() {
  const [toasts, setToasts] = useState<Toast[]>([])
  useEffect(() => {
    const es = new EventSource('/api/realtime/orders')
    let n = 0
    es.addEventListener('order', (e) => {
      const d = JSON.parse((e as MessageEvent).data) as {
        event: string
        order: { symbol: string; side: string; quantity: string; execution?: { filledQuantity?: string; averageFilledPrice?: string | null } }
      }
      const [label, tone] = EVENT_TEXT[d.event] ?? [d.event, 'ok']
      if (d.event === 'CANCELING' || d.event === 'REPLACING') return
      const o = d.order
      const filled = d.event.includes('FILL') && o.execution?.filledQuantity ? ` ${o.execution.filledQuantity}주 @ ${Number(o.execution.averageFilledPrice).toLocaleString()}` : ''
      const toast = { id: ++n, tone, text: `${o.symbol} ${o.side === 'BUY' ? '매수' : '매도'} ${label}${filled}` }
      setToasts((t) => [...t.slice(-3), toast])
      window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== toast.id)), 8000)
      window.dispatchEvent(new Event('mt:order-event'))
    })
    es.onerror = () => {
      if (es.readyState === EventSource.CLOSED) {
        es.close() // no Toss key / session expired: verify session once so expired tabs redirect cleanly
        authApi.me().catch(() => undefined)
      }
    }
    return () => es.close()
  }, [])
  return toasts
}
