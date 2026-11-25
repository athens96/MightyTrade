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

    expect(getParamHint('vwap_pullback', 'ema_n', '단기 추세 EMA', 3, 30)).toContain('9봉')
    expect(getParamHint('hod_breakout', 'tight_pct', '고가놀이 허용폭 (%)', 0.3, 5)).toContain('고가놀이')
    expect(getParamHint('red_to_green', 'min_dip_pct', '초반 음봉 눌림 최소 (%)', 0.1, 5)).toContain('파란불')
    expect(getParamHint('closing_bet', 'close_strength', '고가 근접도 (0~1)', 0.5, 0.98)).toContain('윗꼬리')
    expect(getParamHint('hp_filter', 'lambda', '부드러움 계수 λ', 100, 10000000)).toContain('부드러움')
    expect(getParamHint('knn', 'train_window', '과거 학습 기간 (일)', 60, 750)).toContain('과거 학습')
    expect(getParamHint('alpha_combo', 'lookback_m', '평가 기간 M (일)', 10, 30)).toContain('36개')
  })

  it('returns portfolio parameter hints across allocation models', () => {
    expect(getParamHint('risk_parity', 'vol_window', '변동성 기간', 20, 252)).toContain('출렁임(변동성)')
    expect(getParamHint('baa', 'canary_threshold', '대피 기준', 1, 4)).toContain('감시병')
    expect(getParamHint('sector_rotation', 'ma_days', '추세 이평', 60, 252)).toContain('이동평균선')
    expect(getParamHint('cov_max_sharpe', 'cov_window', '공분산 측정 기간 (일)', 30, 252)).toContain('공분산')
  })

  it('falls back cleanly for unknown parameters', () => {
    expect(getParamHint('unknown_model', 'custom_k', '사용자 배수', 1, 5)).toBe('사용자 배수 설정값입니다. (입력 가능 범위: 1 ~ 5)')
  })
})
