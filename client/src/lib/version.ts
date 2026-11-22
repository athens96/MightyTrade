declare const __APP_VERSION__: string | undefined

export const APP_VERSION: string = typeof __APP_VERSION__ === 'string' && __APP_VERSION__ ? __APP_VERSION__ : '0.1.0'

export interface HealthInfo {
  status: string
  version?: string
}

const SEMVER_RE = /^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?$/

export function parseSemver(v: string | null | undefined): [number, number, number] | null {
  if (!v) return null
  const m = SEMVER_RE.exec(v.trim())
  if (!m) return null
  return [Number(m[1]), Number(m[2]), Number(m[3])]
}

export function compareSemver(a: string, b: string): number {
  const pa = parseSemver(a)
  const pb = parseSemver(b)
  if (!pa || !pb) return 0
  for (let i = 0; i < 3; i++) {
    if (pa[i] !== pb[i]) return pa[i] > pb[i] ? 1 : -1
  }
  return 0
}

export function isNewerVersion(remote: string | null | undefined, current: string = APP_VERSION): boolean {
  if (!remote || !parseSemver(remote) || !parseSemver(current)) return false
  return compareSemver(remote, current) > 0
}

export async function fetchHealth(): Promise<HealthInfo> {
  const res = await fetch('/api/health', {
    cache: 'no-store',
    credentials: 'same-origin',
    headers: { Pragma: 'no-cache' },
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return (await res.json()) as HealthInfo
}

export const DISMISSED_VER_KEY = 'mt.version.dismissed'
export const RELOADED_VER_KEY = 'mt.version.reloadedFor'

/** Strip temporary cache-busting query params (`_v`, `_t`) after a forced reload so URLs stay clean. */
export function stripReloadParams(): void {
  if (typeof window === 'undefined') return
  const url = new URL(window.location.href)
  if (!url.searchParams.has('_v') && !url.searchParams.has('_t')) return
  url.searchParams.delete('_v')
  url.searchParams.delete('_t')
  window.history.replaceState(window.history.state, '', `${url.pathname}${url.search}${url.hash}`)
}

/** Bypass browser HTML/asset caches and force-load the newly deployed web bundle. */
export async function forceReloadApp(targetVersion?: string): Promise<void> {
  if (typeof window === 'undefined') return
  try {
    if (targetVersion) sessionStorage.setItem(RELOADED_VER_KEY, targetVersion)
    sessionStorage.removeItem(DISMISSED_VER_KEY)
  } catch {
    // ignore storage errors
  }

  try {
    if ('caches' in window) {
      const keys = await window.caches.keys()
      await Promise.all(keys.map((k) => window.caches.delete(k)))
    }
  } catch {
    // ignore CacheStorage errors
  }

  try {
    await Promise.all([
      fetch('/', { cache: 'reload', credentials: 'same-origin' }),
      fetch(window.location.pathname, { cache: 'reload', credentials: 'same-origin' }),
    ])
  } catch {
    // ignore network errors and proceed with navigation reload
  }

  const url = new URL(window.location.href)
  url.searchParams.set('_v', targetVersion ?? String(Date.now()))
  url.searchParams.set('_t', String(Date.now()))
  window.location.replace(url.toString())
}
