import { describe, expect, it } from 'vitest'
import { getParamHint } from './paramHints'

describe('getParamHint', () => {
  it('returns strategy-specific beginner-friendly Korean hints with bounds', () => {
    const maFast = getParamHint('ma_cross', 'fast', '단기 이평', 3, 100)
    expect(maFast).toContain('단기 평균선 기간')
    expect(maFast).toContain('3 ~ 100')

    const rsiEntry = getParamHint('rsi2', 'entry', '진입 기준', 2, 30)
    expect(rsiEntry).toContain('RSI가 이 숫자 아래로')
    expect(rsiEntry).toContain('2 ~ 30')

    const orbBars = getParamHint('orb_breakout', 'open_bars', '오프닝 봉 수', 1, 6)
    expect(orbBars).toContain('15분봉')
  })

  it('returns portfolio parameter hints across allocation models', () => {
    expect(getParamHint('risk_parity', 'vol_window', '변동성 기간', 20, 252)).toContain('출렁임(변동성)')
    expect(getParamHint('baa', 'canary_threshold', '대피 기준', 1, 4)).toContain('감시병')
    expect(getParamHint('sector_rotation', 'ma_days', '추세 이평', 60, 252)).toContain('이동평균선')
  })

  it('falls back cleanly for unknown parameters', () => {
    expect(getParamHint('unknown_model', 'custom_k', '사용자 배수', 1, 5)).toBe('사용자 배수 설정값입니다. (입력 가능 범위: 1 ~ 5)')
  })
})
