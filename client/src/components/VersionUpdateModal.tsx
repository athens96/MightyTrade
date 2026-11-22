import { useCallback, useEffect, useRef, useState } from 'react'
import {
  APP_VERSION,
  DISMISSED_VER_KEY,
  RELOADED_VER_KEY,
  fetchHealth,
  forceReloadApp,
  isNewerVersion,
  stripReloadParams,
} from '../lib/version'

const POLL_MS = 60_000
const WAKE_MIN_MS = 15_000
const CONFIRM_DELAY_MS = 2_500

export const OPEN_UPDATE_MODAL_EVENT = 'mt:open-update-modal'
export const UPDATE_AVAILABLE_EVENT = 'mt:update-available'

export default function VersionUpdateModal() {
  const [latest, setLatest] = useState<string | null>(null)
  const [open, setOpen] = useState(false)
  const [reloading, setReloading] = useState(false)
  const lastCheckAt = useRef(0)
  const confirmTimer = useRef<number | undefined>(undefined)

  const checkVersion = useCallback(async (forceOpen = false) => {
    lastCheckAt.current = Date.now()
    try {
      const h = await fetchHealth()
      const remote = h.version?.trim()
      if (!remote || !isNewerVersion(remote, APP_VERSION)) return

      const announce = (ver: string) => {
        setLatest(ver)
        window.dispatchEvent(new CustomEvent(UPDATE_AVAILABLE_EVENT, { detail: { version: ver } }))
        if (forceOpen) {
          setOpen(true)
          return
        }
        try {
          if (sessionStorage.getItem(DISMISSED_VER_KEY) === ver) return
          if (sessionStorage.getItem(RELOADED_VER_KEY) === ver) return
        } catch {
          // ignore storage access issues
        }
        setOpen(true)
      }

      if (forceOpen) {
        announce(remote)
        return
      }

      // Wait briefly and re-verify once so static assets finish uploading during deployment
      window.clearTimeout(confirmTimer.current)
      confirmTimer.current = window.setTimeout(async () => {
        try {
          const h2 = await fetchHealth()
          const confirmed = h2.version?.trim()
          if (confirmed && isNewerVersion(confirmed, APP_VERSION)) {
            announce(confirmed)
          }
        } catch {
          announce(remote)
        }
      }, CONFIRM_DELAY_MS)
    } catch {
      // ignore transient offline / deploy-restart hiccups
    }
  }, [])

  useEffect(() => {
    stripReloadParams()
    const initTimer = window.setTimeout(() => {
      void checkVersion(false)
    }, 2_000)

    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void checkVersion(false)
      }
    }, POLL_MS)

    const onWake = () => {
      if (document.visibilityState !== 'visible') return
      if (Date.now() - lastCheckAt.current < WAKE_MIN_MS) return
      void checkVersion(false)
    }

    const onPreloadError = (e: Event) => {
      // Lazy route chunk rotated after a new deploy -> prompt clean force-reload instead of crashing
      e.preventDefault()
      void checkVersion(true)
    }

    const onManualOpen = () => {
      if (latest) setOpen(true)
      else void checkVersion(true)
    }

    document.addEventListener('visibilitychange', onWake)
    window.addEventListener('focus', onWake)
    window.addEventListener('vite:preloadError', onPreloadError)
    window.addEventListener(OPEN_UPDATE_MODAL_EVENT, onManualOpen)

    return () => {
      window.clearTimeout(initTimer)
      window.clearTimeout(confirmTimer.current)
      window.clearInterval(interval)
      document.removeEventListener('visibilitychange', onWake)
      window.removeEventListener('focus', onWake)
      window.removeEventListener('vite:preloadError', onPreloadError)
      window.removeEventListener(OPEN_UPDATE_MODAL_EVENT, onManualOpen)
    }
  }, [checkVersion, latest])

  const dismiss = useCallback(() => {
    if (latest) {
      try {
        sessionStorage.setItem(DISMISSED_VER_KEY, latest)
      } catch {
        // ignore
      }
    }
    setOpen(false)
  }, [latest])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') dismiss()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, dismiss])

  if (!open || !latest) return null

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="ver-update-title" onMouseDown={(e) => e.target === e.currentTarget && dismiss()}>
      <div className="modal" style={{ maxWidth: 420 }}>
        <h3 id="ver-update-title">🔄 새 버전이 출시되었습니다 (v{latest})</h3>
        <p className="small" style={{ margin: '4px 0', lineHeight: 1.6 }}>
          현재 열려 있는 웹 화면은 <b>v{APP_VERSION}</b>이며, 서버에 최신 버전(<b>v{latest}</b>)이 반영되었습니다.
          최신 기능과 화면 개선 사항을 적용하려면 웹을 새로 불러와 주세요.
        </p>
        <p className="muted small" style={{ margin: 0 }}>
          주문이나 봇 설정을 입력 중이라면 <b>나중에</b>를 눌러 작업을 마친 뒤 상단 버전 배지를 눌러 새로고침할 수 있습니다.
        </p>
        <div className="modal-actions" style={{ marginTop: 6 }}>
          <button
            type="button"
            disabled={reloading}
            title="지금은 창을 닫고 현재 작업을 계속합니다 (상단 버전 표시를 누르면 언제든 다시 새로고침할 수 있습니다)"
            onClick={dismiss}
          >
            나중에 (취소)
          </button>
          <button
            type="button"
            className="primary"
            disabled={reloading}
            title={`브라우저 이전 캐시를 비우고 최신 버전(v${latest}) 웹 화면을 즉시 다시 불러옵니다`}
            onClick={() => {
              setReloading(true)
              void forceReloadApp(latest)
            }}
          >
            {reloading ? '새 버전 불러오는 중…' : '지금 새로고침'}
          </button>
        </div>
      </div>
    </div>
  )
}
