import { afterEach, describe, expect, it, vi } from 'vitest'
import { api, ApiError, isSessionExpiredError, onSessionExpired } from './client'

describe('api session-expiration handling', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('notifies session-expired listeners on 401 unauthorized / not_approved / otp_required', async () => {
    const events: { code: string; path: string }[] = []
    const unsub = onSessionExpired((e) => events.push(e))

    for (const code of ['unauthorized', 'not_approved', 'otp_required']) {
      vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(
        new Response(JSON.stringify({ error: code, message: '만료됨' }), {
          status: 401,
          headers: { 'Content-Type': 'application/json' },
        }),
      )
      await expect(api('/autotrade/bots')).rejects.toThrow(ApiError)
    }

    unsub()
    expect(events).toEqual([
      { code: 'unauthorized', path: '/autotrade/bots' },
      { code: 'not_approved', path: '/autotrade/bots' },
      { code: 'otp_required', path: '/autotrade/bots' },
    ])
  })

  it('does NOT trigger session expiration on wrong OTP (401 invalid_otp) or 403 trading_locked', async () => {
    const events: { code: string; path: string }[] = []
    const unsub = onSessionExpired((e) => events.push(e))

    vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(
      new Response(JSON.stringify({ error: 'invalid_otp', message: 'OTP 오류' }), {
        status: 401,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    let caught: unknown
    try {
      await api('/trading/unlock', { method: 'POST', json: { otp_code: '000000' } })
    } catch (err) {
      caught = err
    }
    expect(isSessionExpiredError(caught)).toBe(false)

    vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(
      new Response(JSON.stringify({ error: 'trading_locked', message: '잠김' }), {
        status: 403,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    await expect(api('/trading/orders', { method: 'POST', json: {} })).rejects.toThrow(ApiError)

    unsub()
    expect(events).toEqual([])
  })
})
