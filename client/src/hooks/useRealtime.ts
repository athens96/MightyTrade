import { useEffect, useState } from 'react'
import { marketApi, type OrderBook, type Tick } from '../api/market'
import type { Market } from '../api/types'

export type StreamState = 'connecting' | 'connected' | 'reconnecting' | 'unavailable'

/** Orderbook + trade ticks: REST snapshot first, then live updates via the server's SSE relay. */
export function useRealtime(market: Market | undefined, symbol: string | undefined) {
  const [book, setBook] = useState<OrderBook | null>(null)
  const [ticks, setTicks] = useState<Tick[]>([]) // newest first
  const [state, setState] = useState<StreamState>('connecting')

  useEffect(() => {
    if (!market || !symbol) return
    let closed = false
    setBook(null)
    setTicks([])
    setState('connecting')
    marketApi
      .orderbook(market, symbol)
      .then((r) => {
        if (closed) return
        setBook(r.orderbook)
        setTicks(r.trades)
      })
      .catch(() => !closed && setState('unavailable'))

    const es = new EventSource(`/api/realtime/stream?${new URLSearchParams({ market, symbol })}`)
    es.addEventListener('status', (e) => setState((JSON.parse((e as MessageEvent).data).state as StreamState) ?? 'connected'))
    es.addEventListener('orderbook', (e) => setBook(JSON.parse((e as MessageEvent).data)))
    es.addEventListener('trade', (e) => {
      const t = JSON.parse((e as MessageEvent).data) as Tick
      setTicks((prev) => [t, ...prev].slice(0, 60))
    })
    es.onerror = () => {
      // EventSource retries by itself; CLOSED means the server refused (e.g. no Toss key / mock provider)
      if (es.readyState === EventSource.CLOSED) setState('unavailable')
      else setState('reconnecting')
    }
    return () => {
      closed = true
      es.close()
    }
  }, [market, symbol])

  return { book, ticks, state }
}
