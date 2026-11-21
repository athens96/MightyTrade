import { useEffect, useRef, useState } from 'react'

interface TipState {
  text: string
  x: number
  y: number
  below: boolean
}

const SHOW_DELAY_MS = 140
const VIEWPORT_PAD = 10

function findTipTarget(node: EventTarget | null): HTMLElement | null {
  if (!(node instanceof Element)) return null
  const el = node.closest('[data-tip], [title]')
  return el instanceof HTMLElement ? el : null
}

function findDisabledTipAtPoint(root: EventTarget | null, x: number, y: number): HTMLElement | null {
  if (!(root instanceof Element)) return null
  const list = root.querySelectorAll(':disabled[data-tip], :disabled[title]')
  for (const item of list) {
    if (!(item instanceof HTMLElement)) continue
    const r = item.getBoundingClientRect()
    if (r.width > 0 && r.height > 0 && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) {
      return item
    }
  }
  return null
}

function readAndPromoteTip(el: HTMLElement): string {
  const nativeTitle = el.getAttribute('title')
  if (nativeTitle !== null) {
    const trimmed = nativeTitle.trim()
    if (trimmed) {
      el.setAttribute('data-tip', trimmed)
      if (!el.getAttribute('aria-label') && (el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA')) {
        el.setAttribute('aria-label', trimmed)
      }
    } else {
      el.removeAttribute('data-tip')
    }
    // Remove native `title` so the slow OS tooltip never overlaps our styled tooltip
    el.removeAttribute('title')
  }
  return (el.getAttribute('data-tip') ?? '').trim()
}

/**
 * Global zero-wrapper tooltip overlay.
 * Any button, input, select, textarea, or label with `title="..."` (or `data-tip="..."`)
 * automatically gets a fast, readable dark tooltip on mouse hover / keyboard focus.
 */
export default function HoverTooltip() {
  const [tip, setTip] = useState<TipState | null>(null)
  const activeEl = useRef<HTMLElement | null>(null)
  const activeText = useRef('')
  const timer = useRef<number | undefined>(undefined)
  const raf = useRef<number | undefined>(undefined)

  useEffect(() => {
    const clear = () => {
      window.clearTimeout(timer.current)
      activeEl.current = null
      activeText.current = ''
      setTip(null)
    }

    const showFor = (el: HTMLElement, immediate = false) => {
      const text = readAndPromoteTip(el)
      if (!text) {
        clear()
        return
      }
      if (activeEl.current === el && activeText.current === text) return
      window.clearTimeout(timer.current)
      activeEl.current = el
      activeText.current = text

      const open = () => {
        if (!el.isConnected || activeEl.current !== el) {
          clear()
          return
        }
        const rect = el.getBoundingClientRect()
        if (rect.width === 0 && rect.height === 0) return
        const cx = Math.min(window.innerWidth - VIEWPORT_PAD - 140, Math.max(VIEWPORT_PAD + 140, rect.left + rect.width / 2))
        const below = rect.top < 64
        const y = below ? rect.bottom + 8 : rect.top - 8
        setTip({ text, x: cx, y, below })
      }

      if (immediate) open()
      else timer.current = window.setTimeout(open, SHOW_DELAY_MS)
    }

    const resolvePointerTarget = (e: PointerEvent): HTMLElement | null =>
      findDisabledTipAtPoint(e.target, e.clientX, e.clientY) ?? findTipTarget(e.target)

    const onPointerOver = (e: PointerEvent) => {
      if (e.pointerType === 'touch') return
      const target = resolvePointerTarget(e)
      if (!target) {
        if (activeEl.current) clear()
        return
      }
      showFor(target)
    }

    const onPointerMove = (e: PointerEvent) => {
      if (e.pointerType === 'touch' || raf.current !== undefined) return
      const evtTarget = e.target
      const { clientX, clientY } = e
      raf.current = window.requestAnimationFrame(() => {
        raf.current = undefined
        const disabledHit = findDisabledTipAtPoint(evtTarget, clientX, clientY)
        if (disabledHit) {
          showFor(disabledHit)
          return
        }
        if (activeEl.current?.matches(':disabled')) {
          const fallback = findTipTarget(evtTarget)
          if (fallback) showFor(fallback, true)
          else clear()
        }
      })
    }

    const onPointerOut = (e: PointerEvent) => {
      if (!activeEl.current || activeEl.current.matches(':disabled')) return
      const next = findTipTarget(e.relatedTarget)
      if (next === activeEl.current) return
      if (next) showFor(next, true)
      else clear()
    }

    const onFocusIn = (e: FocusEvent) => {
      // Only show on keyboard focus (:focus-visible) so clicking an input doesn't cover text
      if (e.target instanceof HTMLElement && !e.target.matches(':focus-visible')) return
      const target = findTipTarget(e.target)
      if (target) showFor(target)
    }

    const onHide = () => {
      if (activeEl.current) clear()
    }

    document.addEventListener('pointerover', onPointerOver, true)
    document.addEventListener('pointermove', onPointerMove, { capture: true, passive: true })
    document.addEventListener('pointerout', onPointerOut, true)
    document.addEventListener('focusin', onFocusIn, true)
    document.addEventListener('focusout', onHide, true)
    document.addEventListener('pointerdown', onHide, true)
    document.addEventListener('scroll', onHide, true)
    document.addEventListener('keydown', onHide, true)
    return () => {
      window.clearTimeout(timer.current)
      if (raf.current !== undefined) window.cancelAnimationFrame(raf.current)
      document.removeEventListener('pointerover', onPointerOver, true)
      document.removeEventListener('pointermove', onPointerMove, true)
      document.removeEventListener('pointerout', onPointerOut, true)
      document.removeEventListener('focusin', onFocusIn, true)
      document.removeEventListener('focusout', onHide, true)
      document.removeEventListener('pointerdown', onHide, true)
      document.removeEventListener('scroll', onHide, true)
      document.removeEventListener('keydown', onHide, true)
    }
  }, [])

  if (!tip) return null

  return (
    <div
      className={`hover-tooltip ${tip.below ? 'below' : 'above'}`}
      role="tooltip"
      style={{ left: tip.x, top: tip.y }}
    >
      {tip.text}
    </div>
  )
}
