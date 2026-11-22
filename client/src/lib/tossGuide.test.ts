import { describe, expect, it } from 'vitest'
import {
  TOSS_FAQ_ITEMS,
  TOSS_GUIDE_STEPS,
  TOSS_QUICK_CARDS,
  TOSS_SECURITY_POINTS,
  tossGuideTargetForField,
} from './tossGuide'

describe('tossGuide', () => {
  it('provides 5 complete sequential steps with unique ids and mockups', () => {
    expect(TOSS_GUIDE_STEPS).toHaveLength(5)
    const ids = new Set(TOSS_GUIDE_STEPS.map((s) => s.id))
    const mockups = new Set(TOSS_GUIDE_STEPS.map((s) => s.mockup))
    expect(ids.size).toBe(5)
    expect(mockups.size).toBe(5)

    for (const step of TOSS_GUIDE_STEPS) {
      expect(step.title.length).toBeGreaterThan(5)
      expect(step.summary.length).toBeGreaterThan(10)
      expect(step.actions.length).toBeGreaterThanOrEqual(3)
    }
  })

  it('maps every form field anchor and quick card to a valid step index', () => {
    const anchors = ['client-id', 'client-secret', 'allowed-ip', 'otp'] as const
    for (const a of anchors) {
      const idx = tossGuideTargetForField(a)
      expect(idx).toBeGreaterThanOrEqual(0)
      expect(idx).toBeLessThan(TOSS_GUIDE_STEPS.length)
    }

    expect(TOSS_QUICK_CARDS).toHaveLength(3)
    for (const card of TOSS_QUICK_CARDS) {
      expect(card.stepIndex).toBeGreaterThanOrEqual(0)
      expect(card.stepIndex).toBeLessThan(TOSS_GUIDE_STEPS.length)
    }
  })

  it('provides security highlights and troubleshooting FAQ entries with valid step links', () => {
    expect(TOSS_SECURITY_POINTS.length).toBeGreaterThanOrEqual(3)
    expect(TOSS_FAQ_ITEMS.length).toBeGreaterThanOrEqual(4)
    for (const item of TOSS_FAQ_ITEMS) {
      expect(item.q.length).toBeGreaterThan(5)
      expect(item.a.length).toBeGreaterThan(10)
      if (item.stepIndex != null) {
        expect(item.stepIndex).toBeGreaterThanOrEqual(0)
        expect(item.stepIndex).toBeLessThan(TOSS_GUIDE_STEPS.length)
      }
    }
  })
})
