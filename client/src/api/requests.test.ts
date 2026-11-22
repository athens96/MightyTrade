import { afterEach, describe, expect, it, vi } from 'vitest'
import { adminApi } from './admin'
import { ApiError, isSessionExpiredError } from './client'
import { REQUEST_CATEGORIES, REQUEST_STATUS_LABEL, requestsApi } from './requests'

describe('user requests & admin-only inbox api', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('submits a user request and fetches only own items via /users/me/requests', async () => {
    const mockItem = {
      id: 'req123',
      category: 'feature',
      title: '다크모드 대비 개선',
      content: '호가창 숫자가 조금 더 선명하면 좋겠습니다.',
      page: '/',
      app_version: '0.1.4',
      status: 'open',
      admin_reply: '',
      replied_at: null,
      created_at: '2026-11-22T10:00:00Z',
      updated_at: '2026-11-22T10:00:00Z',
    }
    const spy = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ item: mockItem }), {
          status: 201,
          headers: { 'Content-Type': 'application/json' },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ items: [mockItem] }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      )

    const created = await requestsApi.create({
      category: 'feature',
      title: mockItem.title,
      content: mockItem.content,
      page: '/',
      app_version: '0.1.4',
    })
    expect(created.item.id).toBe('req123')
    expect(spy.mock.calls[0]?.[0]).toBe('/api/users/me/requests')

    const mine = await requestsApi.listMine()
    expect(mine.items).toHaveLength(1)
    expect(REQUEST_CATEGORIES.map((c) => c.id)).toEqual(['feature', 'bug', 'strategy', 'other'])
    expect(REQUEST_STATUS_LABEL.open).toBe('접수 대기')
  })

  it('surfaces 403 admin_only without logging the user out if non-admin calls admin inbox', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(
      new Response(JSON.stringify({ error: 'admin_only', message: '관리자 전용 기능입니다' }), {
        status: 403,
        headers: { 'Content-Type': 'application/json' },
      }),
    )

    let caught: unknown
    try {
      await adminApi.requests('open')
    } catch (err) {
      caught = err
    }
    expect(caught).toBeInstanceOf(ApiError)
    expect((caught as ApiError).code).toBe('admin_only')
    expect(isSessionExpiredError(caught)).toBe(false)
  })
})
