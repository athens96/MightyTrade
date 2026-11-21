// Rule-based multi-indicator chart diagnosis engine.
// Evaluates trend, overbought/oversold timing, volume/money-flow, and support/resistance
// directly from OHLCV candles in <1ms and translates them into beginner-friendly Korean.

import type { Candle, Interval, Market } from '../api/types'
import * as calc from './calc'
import type { Bars } from './calc'
import { highest, lowest, sma } from './math'

export type SignalTone = 'bull' | 'bear' | 'warn' | 'cool' | 'neutral'
export type OverallTone = 'bull' | 'overheated' | 'pullback' | 'neutral' | 'oversold_bear' | 'bear'

export interface IndicatorSignal {
  id: string
  category: 'trend' | 'timing' | 'volume'
  name: string
  valueText: string
  status: string
  detail: string
  tone: SignalTone
}

export interface ChartDiagnosisResult {
  intervalLabel: string
  score: number // 0 ~ 100 overall technical health
  trendScore: number // 0 ~ 100
  volumeScore: number // 0 ~ 100
  heatLabel: string // 과열 / 따뜻함 / 적정 / 눌림 / 과매도
  overallTone: OverallTone
  badge: string
  headline: string
  summary: string
  tip: string
  support: { price: number; label: string } | null
  resistance: { price: number; label: string } | null
  atr: { value: number; pct: number } | null
  squeeze: boolean
  signals: IndicatorSignal[]
}

const INTERVAL_NAMES: Record<Interval, string> = {
  '1m': '1분봉(초단기)',
  '5m': '5분봉(단타)',
  '15m': '15분봉(단타)',
  '1h': '1시간봉(단기)',
  '1d': '일봉(스윙·중기)',
  '1w': '주봉(장기)',
}

const tzFmt: Record<string, Intl.DateTimeFormat> = {}
function sessionKeyOf(market: Market) {
  const tz = market === 'KR' ? 'Asia/Seoul' : 'America/New_York'
  tzFmt[tz] ??= new Intl.DateTimeFormat('en-CA', { timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit' })
  return (t: number) => tzFmt[tz].format(new Date(t * 1000))
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v))

export function diagnoseChart(
  candles: Candle[],
  market: Market,
  interval: Interval,
  livePrice?: number | null,
): ChartDiagnosisResult | null {
  if (!candles || candles.length < 25) return null

  const n = candles.length
  const lastIdx = n - 1
  const b: Bars = {
    time: candles.map((c) => c.time),
    open: candles.map((c) => c.open),
    high: candles.map((c, i) => (i === lastIdx && livePrice ? Math.max(c.high, livePrice) : c.high)),
    low: candles.map((c, i) => (i === lastIdx && livePrice ? Math.min(c.low, livePrice) : c.low)),
    close: candles.map((c, i) => (i === lastIdx && livePrice ? livePrice : c.close)),
    volume: candles.map((c) => c.volume),
  }

  const price = b.close[lastIdx]
  if (!price || price <= 0) return null
  const intraday = interval !== '1d' && interval !== '1w'
  const unitLabel = interval === '1d' ? '일' : interval === '1w' ? '주' : '봉'

  const signals: IndicatorSignal[] = []
  let trendPts = 0
  let trendMax = 0
  let volPts = 0
  let volMax = 0
  let heatScore = 0 // positive = hot/overbought, negative = cold/oversold

  // ------------------------------------------------------------------ 1. Trend indicators
  const ma5 = sma(b.close, 5)
  const ma20 = sma(b.close, 20)
  const ma60 = sma(b.close, 60)
  const m5 = ma5[lastIdx]
  const m20 = ma20[lastIdx]
  const m60 = ma60[lastIdx]

  if (m5 !== null && m20 !== null) {
    trendMax += 2.5
    const prev5 = ma5[lastIdx - 1]
    const prev20 = ma20[lastIdx - 1]
    const goldenCross = prev5 !== null && prev20 !== null && prev5 <= prev20 && m5 > m20
    const deadCross = prev5 !== null && prev20 !== null && prev5 >= prev20 && m5 < m20

    if (m60 !== null && price > m5 && m5 > m20 && m20 > m60) {
      trendPts += 2.5
      signals.push({
        id: 'ma',
        category: 'trend',
        name: '이동평균선 배열',
        valueText: `5 > 20 > 60${unitLabel}선`,
        status: '정배열 상승',
        detail: `단기·중기·장기 평균선이 아래에서 계단처럼 탄탄히 받쳐주는 전형적인 상승 흐름입니다.`,
        tone: 'bull',
      })
    } else if (m60 !== null && price < m5 && m5 < m20 && m20 < m60) {
      trendPts -= 2.5
      signals.push({
        id: 'ma',
        category: 'trend',
        name: '이동평균선 배열',
        valueText: `5 < 20 < 60${unitLabel}선`,
        status: '역배열 하락',
        detail: `평균선들이 현재 가격 위에서 무겁게 누르고 있는 약세 흐름입니다.`,
        tone: 'bear',
      })
    } else if (goldenCross) {
      trendPts += 2.0
      signals.push({
        id: 'ma',
        category: 'trend',
        name: '이동평균선 교차',
        valueText: `5${unitLabel}선 상향 돌파`,
        status: '골든크로스',
        detail: `방금 단기 평균선이 20${unitLabel} 평균선을 위로 뚫고 올라서며 상승 전환 신호가 켜졌습니다.`,
        tone: 'bull',
      })
    } else if (deadCross) {
      trendPts -= 2.0
      signals.push({
        id: 'ma',
        category: 'trend',
        name: '이동평균선 교차',
        valueText: `5${unitLabel}선 하향 이탈`,
        status: '데드크로스',
        detail: `방금 단기 평균선이 20${unitLabel} 평균선 아래로 꺾이며 단기 약세 신호가 켜졌습니다.`,
        tone: 'bear',
      })
    } else if (price >= m20) {
      trendPts += 1.2
      signals.push({
        id: 'ma',
        category: 'trend',
        name: '이동평균선 위치',
        valueText: `20${unitLabel}선 위`,
        status: '중기선 안착',
        detail: `최근 20${unitLabel} 평균 가격보다 위에서 거래되어 기본 흐름이 양호합니다.`,
        tone: 'bull',
      })
    } else {
      trendPts -= 1.2
      signals.push({
        id: 'ma',
        category: 'trend',
        name: '이동평균선 위치',
        valueText: `20${unitLabel}선 아래`,
        status: '중기선 이탈',
        detail: `최근 20${unitLabel} 평균 가격보다 아래로 내려와 힘이 다소 약해진 상태입니다.`,
        tone: 'bear',
      })
    }
  }

  // MACD (12, 26, 9)
  const macdOut = calc.macd(b.close, 12, 26, 9)
  const mc = macdOut.macd[lastIdx]
  const ms = macdOut.signal[lastIdx]
  const mh = macdOut.hist[lastIdx]
  const prevMh = macdOut.hist[lastIdx - 1]
  if (mc !== null && ms !== null && mh !== null) {
    trendMax += 2.0
    const crossUp = prevMh !== null && prevMh <= 0 && mh > 0
    const crossDn = prevMh !== null && prevMh >= 0 && mh < 0
    if (crossUp) {
      trendPts += 2.0
      signals.push({
        id: 'macd',
        category: 'trend',
        name: 'MACD 추세 속도',
        valueText: '상향 돌파',
        status: '상승 전환',
        detail: 'MACD 선이 신호선을 위로 뚫으며 상승 탄력이 다시 붙기 시작했습니다.',
        tone: 'bull',
      })
    } else if (crossDn) {
      trendPts -= 2.0
      signals.push({
        id: 'macd',
        category: 'trend',
        name: 'MACD 추세 속도',
        valueText: '하향 이탈',
        status: '하락 전환',
        detail: 'MACD 선이 신호선 아래로 내려가며 상승 탄력이 꺾였습니다.',
        tone: 'bear',
      })
    } else if (mh > 0) {
      const growing = prevMh !== null && mh >= prevMh
      trendPts += growing ? 1.6 : 0.8
      signals.push({
        id: 'macd',
        category: 'trend',
        name: 'MACD 추세 속도',
        valueText: growing ? '양수 확대' : '양수 둔화',
        status: growing ? '상승 가속' : '상승 숨고르기',
        detail: growing ? '오르는 속도가 점점 강해지고 있습니다.' : '상승 흐름은 유지 중이나 오르는 속도는 조금 완만해졌습니다.',
        tone: 'bull',
      })
    } else {
      const recovering = prevMh !== null && mh > prevMh
      trendPts += recovering ? -0.6 : -1.6
      signals.push({
        id: 'macd',
        category: 'trend',
        name: 'MACD 추세 속도',
        valueText: recovering ? '음수 축소' : '음수 확대',
        status: recovering ? '하락 진정' : '하락 우세',
        detail: recovering ? '내려가는 힘이 조금씩 약해지며 바닥을 모색 중입니다.' : '아직 내려가는 속도가 우세한 구간입니다.',
        tone: recovering ? 'neutral' : 'bear',
      })
    }
  }

  // Ichimoku Cloud (9, 26, 52) — cloud under current bar comes from index (lastIdx - 25)
  if (n >= 55) {
    const ich = calc.ichimoku(b, 9, 26, 52)
    const cloudIdx = lastIdx - 25
    const sa = cloudIdx >= 0 ? ich.spanA[cloudIdx] : null
    const sb = cloudIdx >= 0 ? ich.spanB[cloudIdx] : null
    if (sa !== null && sb !== null) {
      trendMax += 1.5
      const top = Math.max(sa, sb)
      const bot = Math.min(sa, sb)
      if (price > top) {
        trendPts += 1.5
        signals.push({
          id: 'ichimoku',
          category: 'trend',
          name: '일목균형표 구름대',
          valueText: '구름 위',
          status: '강세 영역',
          detail: '주가가 매물대(구름) 위에 올라서 있어 아래쪽 지지력이 든든한 편입니다.',
          tone: 'bull',
        })
      } else if (price < bot) {
        trendPts -= 1.5
        signals.push({
          id: 'ichimoku',
          category: 'trend',
          name: '일목균형표 구름대',
          valueText: '구름 아래',
          status: '약세 영역',
          detail: '주가 머리 위에 매물대(구름)가 자리 잡아 올라갈 때 저항을 받을 수 있습니다.',
          tone: 'bear',
        })
      } else {
        signals.push({
          id: 'ichimoku',
          category: 'trend',
          name: '일목균형표 구름대',
          valueText: '구름 내부',
          status: '방향 탐색',
          detail: '매물대(구름) 한가운데를 지나는 중이라 위아래 방향 싸움이 치열합니다.',
          tone: 'neutral',
        })
      }
    }
  }

  // DMI / ADX (14, 14)
  const dmiOut = calc.dmi(b, 14, 14)
  const pdi = dmiOut.plusDI[lastIdx]
  const mdi = dmiOut.minusDI[lastIdx]
  const adx = dmiOut.adx[lastIdx]
  if (pdi !== null && mdi !== null && adx !== null) {
    trendMax += 1.5
    const strong = adx >= 25
    const weak = adx < 18
    if (weak) {
      signals.push({
        id: 'dmi',
        category: 'trend',
        name: '추세 강도 (ADX)',
        valueText: `ADX ${adx.toFixed(0)}`,
        status: '박스권 횡보',
        detail: '한쪽으로 쭉 뻗는 추세 힘이 약해 위아래 박스권으로 움직이기 쉬운 구간입니다.',
        tone: 'neutral',
      })
    } else if (pdi > mdi) {
      trendPts += strong ? 1.5 : 0.8
      signals.push({
        id: 'dmi',
        category: 'trend',
        name: '추세 강도 (ADX)',
        valueText: `ADX ${adx.toFixed(0)} · 매수 우위`,
        status: strong ? '강한 상승세' : '완만한 상승세',
        detail: strong ? '사려는 힘이 파는 힘을 압도하며 뚜렷한 추세를 만들고 있습니다.' : '사려는 쪽이 우세하게 흐름을 이끄는 중입니다.',
        tone: 'bull',
      })
    } else {
      trendPts -= strong ? 1.5 : 0.8
      signals.push({
        id: 'dmi',
        category: 'trend',
        name: '추세 강도 (ADX)',
        valueText: `ADX ${adx.toFixed(0)} · 매도 우위`,
        status: strong ? '강한 하락세' : '완만한 하락세',
        detail: strong ? '파는 힘이 강하게 이어지고 있어 성급한 반대 매매는 조심해야 합니다.' : '파는 쪽 힘이 조금 더 우세한 구간입니다.',
        tone: 'bear',
      })
    }
  }

  // Parabolic SAR
  const sarSeries = calc.parabolicSar(b, 0.02, 0.2)
  const sarVal = sarSeries[lastIdx]
  if (sarVal !== null) {
    trendMax += 1.0
    if (price >= sarVal) {
      trendPts += 1.0
      signals.push({
        id: 'psar',
        category: 'trend',
        name: '파라볼릭 SAR',
        valueText: '점 위 유지',
        status: '상승 유지',
        detail: '추세 유도선(점)이 캔들 아래에서 받쳐주며 상승 흐름을 이어가고 있습니다.',
        tone: 'bull',
      })
    } else {
      trendPts -= 1.0
      signals.push({
        id: 'psar',
        category: 'trend',
        name: '파라볼릭 SAR',
        valueText: '점 아래',
        status: '하락 압력',
        detail: '추세 유도선(점)이 캔들 위에 놓여 단기 하락 압력이 남아 있습니다.',
        tone: 'bear',
      })
    }
  }

  // ------------------------------------------------------------------ 2. Timing / Overheat vs Oversold
  const rsiSeries = calc.rsi(b.close, 14)
  const rsiVal = rsiSeries[lastIdx]
  if (rsiVal !== null) {
    if (rsiVal >= 70) {
      heatScore += 2
      signals.push({
        id: 'rsi',
        category: 'timing',
        name: 'RSI (과열·침체)',
        valueText: `${rsiVal.toFixed(0)} / 100`,
        status: '단기 과열',
        detail: '최근 단기간에 많이 올라 피로감이 쌓였습니다. 급하게 추격 매수하면 쉬어가는 구간에 물릴 수 있습니다.',
        tone: 'warn',
      })
    } else if (rsiVal <= 30) {
      heatScore -= 2
      signals.push({
        id: 'rsi',
        category: 'timing',
        name: 'RSI (과열·침체)',
        valueText: `${rsiVal.toFixed(0)} / 100`,
        status: '단기 과매도',
        detail: '단기간에 매도가 몰려 과하게 빠진 바닥권입니다. 기술적 반등이 나올 가능성을 볼 수 있습니다.',
        tone: 'cool',
      })
    } else if (rsiVal >= 55) {
      heatScore += 0.5
      signals.push({
        id: 'rsi',
        category: 'timing',
        name: 'RSI (과열·침체)',
        valueText: `${rsiVal.toFixed(0)} / 100`,
        status: '건강한 강세',
        detail: '과열되지 않은 적정 범위에서 매수 힘이 기분 좋게 앞서고 있습니다.',
        tone: 'bull',
      })
    } else if (rsiVal <= 45) {
      heatScore -= 0.5
      signals.push({
        id: 'rsi',
        category: 'timing',
        name: 'RSI (과열·침체)',
        valueText: `${rsiVal.toFixed(0)} / 100`,
        status: '다소 약세',
        detail: '매수보다 매도 쪽으로 무게가 살짝 기울어 있으나 극단적인 침체는 아닙니다.',
        tone: 'bear',
      })
    } else {
      signals.push({
        id: 'rsi',
        category: 'timing',
        name: 'RSI (과열·침체)',
        valueText: `${rsiVal.toFixed(0)} / 100`,
        status: '균형 (중립)',
        detail: '과열도 침체도 아닌 편안한 중간 영역입니다.',
        tone: 'neutral',
      })
    }
  }

  // Bollinger Bands (20, 2) + Squeeze check
  const bb = calc.bollinger(b.close, 20, 2)
  const bbU = bb.upper[lastIdx]
  const bbM = bb.mid[lastIdx]
  const bbL = bb.lower[lastIdx]
  let squeeze = false
  if (bbU !== null && bbM !== null && bbL !== null && bbU > bbL && bbM > 0) {
    const bwNow = (bbU - bbL) / bbM
    const recentBws: number[] = []
    for (let j = Math.max(0, lastIdx - 59); j <= lastIdx; j++) {
      const u = bb.upper[j]
      const m = bb.mid[j]
      const l = bb.lower[j]
      if (u !== null && m !== null && l !== null && m > 0) recentBws.push((u - l) / m)
    }
    if (recentBws.length >= 20) {
      const sortedBw = [...recentBws].sort((a, c) => a - c)
      squeeze = bwNow <= sortedBw[Math.floor(sortedBw.length * 0.15)]
    }

    const pctB = ((price - bbL) / (bbU - bbL)) * 100
    if (price > bbU) {
      heatScore += 1.5
      signals.push({
        id: 'bb',
        category: 'timing',
        name: '볼린저 밴드',
        valueText: `상단 돌파 (${pctB.toFixed(0)}%)`,
        status: '상단선 돌파',
        detail: '평소 움직이는 범위의 천장을 뚫었습니다. 강한 분출이지만 단기 되돌림도 잦은 자리입니다.',
        tone: 'warn',
      })
    } else if (price < bbL) {
      heatScore -= 1.5
      signals.push({
        id: 'bb',
        category: 'timing',
        name: '볼린저 밴드',
        valueText: `하단 이탈 (${pctB.toFixed(0)}%)`,
        status: '하단선 이탈',
        detail: '평소 범위의 바닥 밖으로 밀려났습니다. 고무줄처럼 안쪽으로 되돌아오려는 힘이 생기기 쉬운 구간입니다.',
        tone: 'cool',
      })
    } else if (squeeze) {
      signals.push({
        id: 'bb',
        category: 'timing',
        name: '볼린저 밴드',
        valueText: `밴드 폭 응축`,
        status: '에너지 응축',
        detail: '밴드 위아래 폭이 최근 들어 가장 좁아졌습니다. 조만간 위든 아래든 큰 움직임이 터질 수 있습니다.',
        tone: 'neutral',
      })
    } else {
      signals.push({
        id: 'bb',
        category: 'timing',
        name: '볼린저 밴드',
        valueText: `밴드 내 ${pctB.toFixed(0)}%`,
        status: price >= bbM ? '중심선 위' : '중심선 아래',
        detail: price >= bbM ? '밴드 위쪽 절반에서 안정적으로 움직이고 있습니다.' : '밴드 아래쪽 절반에 머물러 있습니다.',
        tone: price >= bbM ? 'bull' : 'bear',
      })
    }
  }

  // Slow Stochastic (14, 3, 3)
  const st = calc.stochasticSlow(b, 14, 3, 3)
  const sk = st.k[lastIdx]
  const sd = st.d[lastIdx]
  if (sk !== null && sd !== null) {
    if (sk >= 80) {
      heatScore += 1.0
      signals.push({
        id: 'stoch',
        category: 'timing',
        name: '스토캐스틱',
        valueText: `%K ${sk.toFixed(0)}`,
        status: '단기 고점권',
        detail: '최근 고점 근처에 바짝 다가서 있어 단기 차익 매물이 나올 수 있습니다.',
        tone: 'warn',
      })
    } else if (sk <= 20) {
      heatScore -= 1.0
      const turnUp = sk > sd
      signals.push({
        id: 'stoch',
        category: 'timing',
        name: '스토캐스틱',
        valueText: `%K ${sk.toFixed(0)}`,
        status: turnUp ? '바닥권 반등 조짐' : '단기 바닥권',
        detail: turnUp ? '바닥권에서 위로 고개를 들기 시작했습니다.' : '최근 가격 범위의 맨 아래쪽 바닥권에 위치해 있습니다.',
        tone: 'cool',
      })
    }
  }

  // MFI (14) — volume-weighted RSI
  const mfiSeries = calc.mfi(b, 14)
  const mfiVal = mfiSeries[lastIdx]
  if (mfiVal !== null) {
    if (mfiVal >= 80) {
      heatScore += 1.0
      signals.push({
        id: 'mfi',
        category: 'timing',
        name: '자금흐름 (MFI)',
        valueText: `${mfiVal.toFixed(0)} / 100`,
        status: '자금 유입 과열',
        detail: '거래대금까지 감안했을 때 단기 매수 열기가 매우 뜨겁습니다.',
        tone: 'warn',
      })
    } else if (mfiVal <= 20) {
      heatScore -= 1.0
      signals.push({
        id: 'mfi',
        category: 'timing',
        name: '자금흐름 (MFI)',
        valueText: `${mfiVal.toFixed(0)} / 100`,
        status: '자금 유출 과대',
        detail: '거래량을 동반한 매도가 한바탕 지나간 과매도 구간입니다.',
        tone: 'cool',
      })
    }
  }

  // ------------------------------------------------------------------ 3. Volume & Intraday VWAP
  const volMa20 = sma(b.volume, 20)[lastIdx]
  const curVol = b.volume[lastIdx]
  const prevClose = b.close[lastIdx - 1] ?? price
  const barUp = price >= prevClose
  if (volMa20 !== null && volMa20 > 0) {
    volMax += 2.0
    const ratio = curVol / volMa20
    if (ratio >= 1.8) {
      volPts += barUp ? 2.0 : -1.5
      signals.push({
        id: 'volume',
        category: 'volume',
        name: '거래량 활력',
        valueText: `평소의 ${ratio.toFixed(1)}배`,
        status: barUp ? '대량 매수 유입' : '대량 매도 출회',
        detail: barUp
          ? '평소보다 훨씬 많은 거래량이 실리며 가격을 끌어올려 신뢰도가 높습니다.'
          : '평소보다 많은 거래량과 함께 밀리고 있어 매물 소화가 필요합니다.',
        tone: barUp ? 'bull' : 'bear',
      })
    } else if (ratio <= 0.55) {
      signals.push({
        id: 'volume',
        category: 'volume',
        name: '거래량 활력',
        valueText: `평소의 ${(ratio * 100).toFixed(0)}%`,
        status: '거래 한산',
        detail: '거래가 조용한 편이라 작은 물량에도 가격이 흔들릴 수 있는 눈치보기 구간입니다.',
        tone: 'neutral',
      })
    } else {
      volPts += barUp ? 0.6 : -0.4
      signals.push({
        id: 'volume',
        category: 'volume',
        name: '거래량 활력',
        valueText: `평소의 ${ratio.toFixed(1)}배`,
        status: '평소 수준',
        detail: '무리 없는 평균 수준의 거래량이 이어지고 있습니다.',
        tone: 'neutral',
      })
    }
  }

  // OBV trend vs 20-bar SMA of OBV
  const obvSeries = calc.obv(b)
  const obvMa20 = sma(obvSeries, 20)[lastIdx]
  const obvNow = obvSeries[lastIdx]
  if (obvNow !== null && obvMa20 !== null) {
    volMax += 1.5
    if (obvNow > obvMa20) {
      volPts += 1.5
      signals.push({
        id: 'obv',
        category: 'volume',
        name: '누적 매집 (OBV)',
        valueText: '20평균 위',
        status: '매집 우위',
        detail: '오르는 날 거래량이 내리는 날보다 꾸준히 많아 물량을 모으는 흐름이 우세합니다.',
        tone: 'bull',
      })
    } else if (obvNow < obvMa20) {
      volPts -= 1.5
      signals.push({
        id: 'obv',
        category: 'volume',
        name: '누적 매집 (OBV)',
        valueText: '20평균 아래',
        status: '매도 우위',
        detail: '내리는 날 거래량이 더 많이 나와 물량이 빠져나가는 흐름입니다.',
        tone: 'bear',
      })
    }
  }

  // Intraday VWAP
  if (intraday) {
    const vwapSeries = calc.vwap(b, sessionKeyOf(market))
    const vw = vwapSeries[lastIdx]
    if (vw !== null && vw > 0) {
      volMax += 1.5
      if (price >= vw) {
        volPts += 1.5
        signals.push({
          id: 'vwap',
          category: 'volume',
          name: '당일 평균체결가 (VWAP)',
          valueText: '평균가 위',
          status: '당일 매수세 우위',
          detail: '오늘 거래된 전체 평균 단가보다 위에 있어 오늘 산 투자자들이 평균적으로 수익 구간입니다.',
          tone: 'bull',
        })
      } else {
        volPts -= 1.5
        signals.push({
          id: 'vwap',
          category: 'volume',
          name: '당일 평균체결가 (VWAP)',
          valueText: '평균가 아래',
          status: '당일 매도세 우위',
          detail: '오늘 거래된 전체 평균 단가보다 아래라 본전 매물이 위에서 누를 수 있습니다.',
          tone: 'bear',
        })
      }
    }
  }

  // ------------------------------------------------------------------ 4. Support / Resistance / ATR
  const atrVal = calc.atr(b, 14)[lastIdx]
  const atrInfo = atrVal !== null && atrVal > 0 ? { value: atrVal, pct: (atrVal / price) * 100 } : null

  const hi20 = highest(b.high, 20)[lastIdx]
  const lo20 = lowest(b.low, 20)[lastIdx]

  const supportCandidates: { price: number; label: string }[] = []
  const resistCandidates: { price: number; label: string }[] = []
  const addLevel = (lv: number | null, label: string) => {
    if (lv === null || !Number.isFinite(lv) || lv <= 0) return
    if (lv < price * 0.998) supportCandidates.push({ price: lv, label })
    else if (lv > price * 1.002) resistCandidates.push({ price: lv, label })
  }
  addLevel(m20, `20${unitLabel} 평균선`)
  addLevel(m60, `60${unitLabel} 평균선`)
  addLevel(bbL, '볼린저 하단')
  addLevel(bbU, '볼린저 상단')
  addLevel(lo20, `최근 20${unitLabel} 최저가`)
  addLevel(hi20, `최근 20${unitLabel} 최고가`)

  supportCandidates.sort((a, c) => c.price - a.price) // closest below current price
  resistCandidates.sort((a, c) => a.price - c.price) // closest above current price
  const support = supportCandidates[0] ?? (lo20 !== null ? { price: lo20, label: `최근 20${unitLabel} 최저가` } : null)
  const resistance = resistCandidates[0] ?? (hi20 !== null ? { price: hi20, label: `최근 20${unitLabel} 최고가` } : null)

  // ------------------------------------------------------------------ 5. Final Scoring & Plain-Korean Synthesis
  const trendNorm = trendMax > 0 ? clamp(((trendPts / trendMax + 1) / 2) * 100, 0, 100) : 50
  const volNorm = volMax > 0 ? clamp(((volPts / volMax + 1) / 2) * 100, 0, 100) : 50
  const rsiNorm = rsiVal !== null ? clamp(rsiVal, 15, 85) : 50
  const score = Math.round(clamp(trendNorm * 0.6 + volNorm * 0.25 + rsiNorm * 0.15, 0, 100))

  const isOverheated = heatScore >= 2.5
  const isOversold = heatScore <= -2.5
  const heatLabel =
    heatScore >= 2.5 ? '단기 과열' : heatScore >= 1.0 ? '매수 열기 높음' : heatScore <= -2.5 ? '단기 과매도(세일)' : heatScore <= -1.0 ? '단기 눌림' : '적정 온도'

  const intervalLabel = INTERVAL_NAMES[interval]
  let overallTone: OverallTone
  let badge: string
  let headline: string
  let summary: string
  let tip: string

  const sideways = adx !== null && adx < 18 && score >= 38 && score <= 62

  if (sideways) {
    overallTone = 'neutral'
    badge = '⚪ 방향 탐색 · 박스권 횡보'
    headline = `${intervalLabel} 기준 뚜렷한 한쪽 방향 없이 힘을 모으는 눈치보기 구간입니다.`
    summary = squeeze
      ? '사려는 힘과 팔려는 힘이 팽팽하고 볼린저 밴드 폭도 좁아져 있어, 조만간 거래량이 터지는 쪽으로 방향이 정해질 가능성이 큽니다.'
      : '평균선 근처에서 위아래로 오르내리는 박스권 흐름입니다. 추세 추종보다는 지지·저항 가격대를 확인하며 차분히 대응하기 좋습니다.'
    tip = '위쪽 저항선을 거래량과 함께 뚫거나 아래쪽 지지선이 깨지기 전까지는 섣부른 추격 매매를 줄이세요.'
  } else if (score >= 63 && isOverheated) {
    overallTone = 'overheated'
    badge = '🟡 상승 강세 · 단기 과열 주의'
    headline = `${intervalLabel} 기준 오르는 힘은 아주 좋지만, 단기간에 많이 올라 숨 고르기가 나올 수 있습니다.`
    summary = '평균선과 추세 지표들은 모두 긍정적이나, RSI·볼린저 등 과열 지표에 경고등이 켜진 상태입니다.'
    tip = '지금 당장 급하게 따라 사기보다는 평균선(지지선) 근처로 살짝 쉬어갈 때 분할로 접근하는 편이 안전합니다.'
  } else if (score >= 63) {
    overallTone = 'bull'
    badge = '🟢 건강한 상승 흐름'
    headline = `${intervalLabel} 기준 평균선과 거래량 흐름이 고르게 받쳐주며 기분 좋게 오르는 중입니다.`
    summary = '추세 방향이 위쪽을 향하고 있고 아직 심한 과열 신호도 없어 상승 흐름이 비교적 탄탄하게 유지되고 있습니다.'
    tip = '아래쪽 주요 지지선(20평균선 등)이 깨지지 않는 한 흐름을 차분히 따라가기 좋은 구간입니다.'
  } else if (score >= 44 && isOversold) {
    overallTone = 'pullback'
    badge = '🔵 큰 흐름 양호 · 단기 눌림목(반등 후보)'
    headline = `${intervalLabel} 기준 전체 틀은 무너지지 않았는데 단기 매도로 과하게 눌린 자리입니다.`
    summary = '최근 단기 지표들이 과매도(세일 구간)를 가리키고 있어, 매도세가 진정되면 고무줄처럼 기술적 반등이 나올 수 있는 구간입니다.'
    tip = '기업에 특별한 악재 뉴스가 없는지 확인한 뒤, 지지선 부근에서 반등 양봉이 나오는지를 함께 체크해 보세요.'
  } else if (score < 44 && isOversold) {
    overallTone = 'oversold_bear'
    badge = '🟠 하락 흐름 속 과매도 (바닥 확인 필요)'
    headline = `${intervalLabel} 기준 추세는 아직 아래쪽이지만 단기간에 너무 많이 빠진 바닥권입니다.`
    summary = '단기 과매도 상태라 일시적인 반등이 나올 수는 있지만, 평균선들이 위에서 누르고 있어 하락세가 완전히 끝났다고 단정하긴 이릅니다.'
    tip = "'많이 떨어졌으니 싸다'고 한 번에 사기보다, 하락이 멈추고 5·20평균선을 다시 회복하는지 먼저 확인하는 것이 안전합니다."
  } else if (score < 40) {
    overallTone = 'bear'
    badge = '🔴 하락 흐름 우세 · 보수적 대응'
    headline = `${intervalLabel} 기준 파는 힘이 더 강하고 평균선들이 위에서 누르는 약세 구간입니다.`
    summary = '추세와 수급 지표 대부분이 아래쪽을 가리키고 있어 반등 시마다 위쪽 매물 저항을 받기 쉬운 자리입니다.'
    tip = '성급한 저점 매수는 미루고, 거래량이 실리며 평균선 위로 올라서는 확실한 전환 신호를 기다리는 편이 유리합니다.'
  } else {
    overallTone = 'neutral'
    badge = '⚪ 중립 · 완만한 눈치보기'
    headline = `${intervalLabel} 기준 매수세와 매도세가 비슷하게 맞서고 있는 균형 구간입니다.`
    summary = '강한 상승도 강한 하락도 아닌 중간 지대입니다. 보조지표 중 한쪽으로 쏠리는 신호가 있는지 함께 살펴보세요.'
    tip = '아래 지지선과 위 저항선 사이에서 분할로 천천히 접근하거나 방향이 뚜렷해질 때 움직이는 것이 좋습니다.'
  }

  return {
    intervalLabel,
    score,
    trendScore: Math.round(trendNorm),
    volumeScore: Math.round(volNorm),
    heatLabel,
    overallTone,
    badge,
    headline,
    summary,
    tip,
    support,
    resistance,
    atr: atrInfo,
    squeeze,
    signals,
  }
}
