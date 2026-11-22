import { describe, expect, it } from 'vitest'
import {
  NOTIFY_CHANNEL_GUIDES,
  NOTIFY_CHANNEL_LIST,
  SAMPLE_ALERT_PREVIEWS,
  guideTargetForField,
} from './notificationGuides'

describe('notificationGuides', () => {
  it('provides step-by-step guides for telegram, discord, and slack with valid links', () => {
    expect(NOTIFY_CHANNEL_LIST.map((c) => c.id)).toEqual(['telegram', 'discord', 'slack'])
    for (const ch of NOTIFY_CHANNEL_LIST) {
      expect(ch.steps.length).toBeGreaterThanOrEqual(4)
      expect(ch.neededFields.length).toBeGreaterThanOrEqual(1)
      for (const step of ch.steps) {
        expect(step.title.length).toBeGreaterThan(3)
        expect(step.actions.length).toBeGreaterThanOrEqual(2)
        for (const link of step.links ?? []) {
          expect(link.url.startsWith('https://')).toBe(true)
        }
      }
    }
  })

  it('deep-links each input field helper to the exact relevant channel and step', () => {
    const tgToken = guideTargetForField('tg-token')
    expect(tgToken).toEqual({ channel: 'telegram', stepIndex: 1 })
    expect(NOTIFY_CHANNEL_GUIDES[tgToken.channel].steps[tgToken.stepIndex].mockup).toBe('tg-newbot-token')

    const tgChat = guideTargetForField('tg-chat-id')
    expect(tgChat).toEqual({ channel: 'telegram', stepIndex: 3 })
    expect(NOTIFY_CHANNEL_GUIDES[tgChat.channel].steps[tgChat.stepIndex].mockup).toBe('tg-get-chatid')

    const dc = guideTargetForField('discord-url')
    expect(dc).toEqual({ channel: 'discord', stepIndex: 1 })
    expect(NOTIFY_CHANNEL_GUIDES[dc.channel].steps[dc.stepIndex].mockup).toBe('dc-channel-integrations')

    const sl = guideTargetForField('slack-url')
    expect(sl).toEqual({ channel: 'slack', stepIndex: 2 })
    expect(NOTIFY_CHANNEL_GUIDES[sl.channel].steps[sl.stepIndex].mockup).toBe('sl-enable-webhooks')
  })

  it('includes realistic sample previews covering order, fill, halt, and daily briefing', () => {
    const kinds = SAMPLE_ALERT_PREVIEWS.map((p) => p.kind)
    expect(kinds).toEqual(['order', 'fill', 'halt', 'briefing'])
    for (const p of SAMPLE_ALERT_PREVIEWS) {
      expect(p.lines.length).toBeGreaterThan(0)
    }
  })
})
