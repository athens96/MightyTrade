import { describe, expect, it } from 'vitest'
import {
  KIWOOM_FAQ_ITEMS,
  KIWOOM_GUIDE_STEPS,
  KIWOOM_QUICK_CARDS,
  KIWOOM_SECURITY_POINTS,
  kiwoomGuideTargetForField,
} from './kiwoomGuide'

describe('kiwoomGuide', () => {
  it('provides 5 complete sequential steps with unique ids and mockups', () => {
    expect(KIWOOM_GUIDE_STEPS).toHaveLength(5)
    expect(new Set(KIWOOM_GUIDE_STEPS.map((s) => s.id)).size).toBe(5)
    expect(new Set(KIWOOM_GUIDE_STEPS.map((s) => s.mockup)).size).toBe(5)
    for (const s of KIWOOM_GUIDE_STEPS) {
      expect(s.title.length).toBeGreaterThan(5)
      expect(s.actions.length).toBeGreaterThanOrEqual(3)
    }
  })

  it('maps every form field anchor and quick card to a valid step index', () => {
    const anchors = ['appkey', 'secretkey', 'allowed-ip', 'mock-mode', 'otp'] as const
    for (const a of anchors) {
      const idx = kiwoomGuideTargetForField(a)
      expect(idx).toBeGreaterThanOrEqual(0)
      expect(idx).toBeLessThan(KIWOOM_GUIDE_STEPS.length)
    }
    expect(KIWOOM_QUICK_CARDS).toHaveLength(3)
    for (const c of KIWOOM_QUICK_CARDS) {
      expect(c.stepIndex).toBeGreaterThanOrEqual(0)
      expect(c.stepIndex).toBeLessThan(KIWOOM_GUIDE_STEPS.length)
    }
  })

  it('provides security highlights and FAQ entries with valid step links', () => {
    expect(KIWOOM_SECURITY_POINTS.length).toBeGreaterThanOrEqual(3)
    expect(KIWOOM_FAQ_ITEMS.length).toBeGreaterThanOrEqual(4)
    for (const item of KIWOOM_FAQ_ITEMS) {
      if (item.stepIndex != null) {
        expect(item.stepIndex).toBeGreaterThanOrEqual(0)
        expect(item.stepIndex).toBeLessThan(KIWOOM_GUIDE_STEPS.length)
      }
    }
  })
})
