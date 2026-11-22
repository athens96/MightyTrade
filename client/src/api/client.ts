// Thin fetch wrapper. Auth is via httpOnly cookies set by the server, so no token handling here.
// When an authenticated session cookie expires (`401 unauthorized` / `not_approved` / `otp_required`),
// listeners registered via `onSessionExpired` transition the app straight back to `/login`.

export class ApiError extends Error {
  constructor(public status: number, public code: string, message: string) {
    super(message)
  }
}

export const SESSION_EXPIRED_CODES = new Set(['unauthorized', 'not_approved', 'otp_required'])
const AUTH_BOOTSTRAP_PATHS = new Set(['/auth/google', '/auth/logout'])

type SessionExpiredListener = (info: { code: string; path: string }) => void
const listeners = new Set<SessionExpiredListener>()

export function onSessionExpired(fn: SessionExpiredListener): () => void {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

export function isSessionExpiredError(err: unknown): err is ApiError {
  return err instanceof ApiError && err.status === 401 && SESSION_EXPIRED_CODES.has(err.code)
}

function notifySessionExpired(code: string, path: string) {
  for (const fn of listeners) {
    try {
      fn({ code, path })
    } catch {
      // ignore listener errors
    }
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent('mt:session-expired', { detail: { code, path } }))
  }
}

export async function api<T>(path: string, init: RequestInit & { json?: unknown } = {}): Promise<T> {
  const { json, headers, ...rest } = init
  const res = await fetch(`/api${path}`, {
    credentials: 'same-origin',
    ...rest,
    headers: { ...(json !== undefined ? { 'Content-Type': 'application/json' } : {}), ...headers },
    body: json !== undefined ? JSON.stringify(json) : rest.body,
  })
  const data = await res.json().catch(() => ({}))
  if (!res.ok) {
    const code = data.error ?? 'error'
    const err = new ApiError(res.status, code, data.message ?? res.statusText)
    if (res.status === 401 && SESSION_EXPIRED_CODES.has(code) && !AUTH_BOOTSTRAP_PATHS.has(path)) {
      notifySessionExpired(code, path)
    }
    throw err
  }
  return data as T
}
