// Indicator catalogue: metadata (KR names, descriptions, default params) + how each one is plotted.
import type { Market } from '../api/types'
import * as calc from './calc'
import type { Bars } from './calc'
import { ema, sma, type Series } from './math'

export type Group = 'overlay' | 'pane'
export type ValueFormat = 'price' | 'number' | 'volume'

export interface ParamDef {
  key: string
  label: string
  default: number
  min: number
  max: number
  step?: number
}

export interface Plot {
  id: string
  label: string
  kind: 'line' | 'histogram' | 'dots'
  values: Series
  color: string
  colors?: (string | null)[] // per-bar colors (histograms)
  offset?: number // plot value i at bar i+offset (Ichimoku spans/lagging line)
  lineWidth?: 1 | 2 | 3
  dashed?: boolean
}

export interface Band {
  upper: string // plot ids
  lower: string
  fillUp: string // fill when upper >= lower
  fillDown: string
}

export interface Level {
  value: number
  color?: string
}

export interface IndicatorOutput {
  plots: Plot[]
  bands?: Band[]
  levels?: Level[]
  format?: ValueFormat
  note?: string // shown instead of values (e.g. VWAP on daily bars)
}

export interface Ctx {
  market: Market
  intraday: boolean
}

export interface IndicatorDef {
  id: string
  name: string
  short: string // legend label
  group: Group
  description: string
  params: ParamDef[]
  compute(b: Bars, p: Record<string, number>, ctx: Ctx): IndicatorOutput
}

export interface IndicatorConfig {
  id: string
  params: Record<string, number>
}

// Up = red, down = blue (KR convention), matching the candles.
export const UP = '#e5484d'
export const DOWN = '#3e63dd'
const MA_COLORS = ['#f5c542', '#ff6b9d', '#4fc3f7', '#b388ff', '#a1887f']

const period = (key: string, label: string, def: number, max = 500): ParamDef => ({ key, label, default: def, min: 1, max })
const maPeriods = (defs: number[]): ParamDef[] =>
  defs.map((d, i) => ({ key: `p${i + 1}`, label: `기간${i + 1}`, default: d, min: 0, max: 500 }))

const tzFormatters: Record<string, Intl.DateTimeFormat> = {}
function sessionKey(market: Market) {
  const tz = market === 'KR' ? 'Asia/Seoul' : 'America/New_York'
  tzFormatters[tz] ??= new Intl.DateTimeFormat('en-CA', { timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit' })
  return (t: number) => tzFormatters[tz].format(new Date(t * 1000))
}

function movingAverages(kind: 'ma' | 'ema', b: Bars, p: Record<string, number>): IndicatorOutput {
  const fn = kind === 'ma' ? sma : ema
  const plots = Object.keys(p)
    .sort()
    .map((k, i) => ({ n: p[k], i }))
    .filter(({ n }) => n > 0)
    .map(({ n, i }) => ({
      id: `${kind}${n}`,
      label: String(n),
      kind: 'line' as const,
      values: fn(b.close, n),
      color: MA_COLORS[i % MA_COLORS.length],
    }))
  return { plots, format: 'price' }
}

export const INDICATORS: IndicatorDef[] = [
  // ------------------------------------------------------------- 가격 지표 (overlay)
  {
    id: 'ma',
    name: '이동평균선 (MA)',
    short: 'MA',
    group: 'overlay',
    description:
      '일정 기간 종가의 단순 평균. 단기선이 장기선 위에 차례로 놓이면(정배열) 상승 추세, 단기선이 장기선을 상향 돌파하면 골든크로스(매수), 하향 돌파하면 데드크로스(매도)로 봅니다. 기간을 0으로 두면 숨깁니다.',
    params: maPeriods([5, 20, 60, 120]),
    compute: (b, p) => movingAverages('ma', b, p),
  },
  {
    id: 'ema',
    name: '지수이동평균 (EMA)',
    short: 'EMA',
    group: 'overlay',
    description: '최근 가격에 더 큰 가중치를 준 이동평균. 단순 이동평균보다 추세 변화에 빠르게 반응합니다.',
    params: maPeriods([12, 26]),
    compute: (b, p) => movingAverages('ema', b, p),
  },
  {
    id: 'bb',
    name: '볼린저 밴드',
    short: 'BB',
    group: 'overlay',
    description:
      '이동평균 ± 표준편차×배수. 밴드 폭이 좁아지면(스퀴즈) 큰 변동이 임박했다는 신호이고, 상단 이탈은 과열·강세, 하단 이탈은 침체·약세로 해석합니다. 가격은 통계적으로 대부분 밴드 안에 머뭅니다.',
    params: [period('n', '기간', 20), { key: 'k', label: '배수', default: 2, min: 0.1, max: 10, step: 0.1 }],
    compute: (b, p) => {
      const r = calc.bollinger(b.close, p.n, p.k)
      return {
        plots: [
          { id: 'upper', label: '상단', kind: 'line', values: r.upper, color: '#8ab4f8' },
          { id: 'mid', label: '중심', kind: 'line', values: r.mid, color: '#f4b400' },
          { id: 'lower', label: '하단', kind: 'line', values: r.lower, color: '#8ab4f8' },
        ],
        bands: [{ upper: 'upper', lower: 'lower', fillUp: 'rgba(138,180,248,0.08)', fillDown: 'rgba(138,180,248,0.08)' }],
        format: 'price',
      }
    },
  },
  {
    id: 'env',
    name: '엔벨로프',
    short: 'ENV',
    group: 'overlay',
    description:
      '이동평균에서 일정 비율(%) 위아래로 그린 밴드. 하단선 근처는 단기 과매도(반등 기대), 상단선 돌파는 강한 상승 추세로 봅니다.',
    params: [period('n', '기간', 20), { key: 'pct', label: '비율(%)', default: 6, min: 0.1, max: 50, step: 0.1 }],
    compute: (b, p) => {
      const r = calc.envelope(b.close, p.n, p.pct)
      return {
        plots: [
          { id: 'upper', label: '상단', kind: 'line', values: r.upper, color: '#80cbc4' },
          { id: 'mid', label: '중심', kind: 'line', values: r.mid, color: '#80cbc4', dashed: true },
          { id: 'lower', label: '하단', kind: 'line', values: r.lower, color: '#80cbc4' },
        ],
        format: 'price',
      }
    },
  },
  {
    id: 'donchian',
    name: '가격 채널 (돈치안)',
    short: 'DC',
    group: 'overlay',
    description:
      '최근 n기간 최고가(상단)와 최저가(하단)를 이은 채널. 상단 돌파는 신고가 돌파로 추세 시작, 하단 이탈은 추세 종료로 봅니다. 터틀 트레이딩의 진입(20일 상단)·청산(10일 하단) 기준입니다.',
    params: [period('upper', '상단 기간', 20), period('lower', '하단 기간', 20)],
    compute: (b, p) => {
      const r = calc.donchian(b, p.upper, p.lower)
      return {
        plots: [
          { id: 'upper', label: '상단', kind: 'line', values: r.upper, color: '#26a69a' },
          { id: 'lower', label: '하단', kind: 'line', values: r.lower, color: '#ef9a9a' },
        ],
        format: 'price',
      }
    },
  },
  {
    id: 'ichimoku',
    name: '일목균형표',
    short: '일목',
    group: 'overlay',
    description:
      '전환선·기준선·선행스팬1/2(구름)·후행스팬으로 구성. 가격이 구름 위면 강세, 아래면 약세, 구름 안은 혼조. 전환선이 기준선을 상향 돌파하면 호전 신호, 구름 두께는 지지·저항의 강도를 뜻합니다. 선행스팬은 미래로, 후행스팬은 과거로 (기준선 기간-1)만큼 이동해 그립니다.',
    params: [period('conv', '전환선', 9), period('base', '기준선', 26), period('spanB', '선행스팬2', 52)],
    compute: (b, p) => {
      const r = calc.ichimoku(b, p.conv, p.base, p.spanB)
      const shift = p.base - 1
      return {
        plots: [
          { id: 'tenkan', label: '전환', kind: 'line', values: r.tenkan, color: '#f06292' },
          { id: 'kijun', label: '기준', kind: 'line', values: r.kijun, color: '#42a5f5' },
          { id: 'chikou', label: '후행', kind: 'line', values: r.chikou, color: '#66bb6a', offset: -shift },
          { id: 'spanA', label: '선행1', kind: 'line', values: r.spanA, color: '#ffb74d', offset: shift, lineWidth: 1 },
          { id: 'spanB', label: '선행2', kind: 'line', values: r.spanB, color: '#9575cd', offset: shift, lineWidth: 1 },
        ],
        bands: [{ upper: 'spanA', lower: 'spanB', fillUp: 'rgba(255,183,77,0.16)', fillDown: 'rgba(149,117,205,0.16)' }],
        format: 'price',
      }
    },
  },
  {
    id: 'psar',
    name: '파라볼릭 SAR',
    short: 'SAR',
    group: 'overlay',
    description:
      '추세를 따라가는 점. 점이 가격 아래에 있으면 상승 추세, 위에 있으면 하락 추세이며 가격이 점을 뚫으면 추세 전환으로 봅니다. 추적 손절가로 많이 씁니다.',
    params: [
      { key: 'step', label: '가속도', default: 0.02, min: 0.001, max: 1, step: 0.001 },
      { key: 'max', label: '최대', default: 0.2, min: 0.01, max: 1, step: 0.01 },
    ],
    compute: (b, p) => ({
      plots: [{ id: 'sar', label: '', kind: 'dots', values: calc.parabolicSar(b, p.step, p.max), color: '#ffd54f' }],
      format: 'price',
    }),
  },
  {
    id: 'vwap',
    name: 'VWAP (분봉)',
    short: 'VWAP',
    group: 'overlay',
    description:
      '당일 거래량 가중 평균가격. 그날 거래된 평균 단가로, 가격이 VWAP 위면 매수세 우위, 아래면 매도세 우위입니다. 단타·기관 매매의 기준선. 분봉에서만 표시됩니다.',
    params: [],
    compute: (b, _p, ctx) =>
      ctx.intraday
        ? { plots: [{ id: 'vwap', label: '', kind: 'line', values: calc.vwap(b, sessionKey(ctx.market)), color: '#e0e0e0', lineWidth: 2 }], format: 'price' }
        : { plots: [], note: '분봉에서만 표시' },
  },

  // ------------------------------------------------------------- 보조 지표 (separate pane)
  {
    id: 'volume',
    name: '거래량',
    short: '거래량',
    group: 'pane',
    description: '기간별 거래 수량과 거래량 이동평균. 가격 돌파가 큰 거래량을 동반하면 신뢰도가 높습니다.',
    params: maPeriods([5, 20]),
    compute: (b, p) => ({
      plots: [
        {
          id: 'vol',
          label: '',
          kind: 'histogram',
          values: b.volume,
          color: UP,
          colors: b.close.map((c, i) => (c >= b.open[i] ? UP + 'aa' : DOWN + 'aa')),
        },
        ...[p.p1, p.p2]
          .filter((n) => n > 0)
          .map((n, i) => ({ id: `vma${n}`, label: `MA${n}`, kind: 'line' as const, values: sma(b.volume, n), color: MA_COLORS[i], lineWidth: 1 as const })),
      ],
      format: 'volume',
    }),
  },
  {
    id: 'macd',
    name: 'MACD',
    short: 'MACD',
    group: 'pane',
    description:
      '단기 EMA와 장기 EMA의 차이(MACD선)와 그 이동평균(시그널선). MACD선이 시그널선을 상향 돌파하면 매수, 하향 돌파하면 매도 신호이며, 0선 위는 상승 추세입니다. 히스토그램은 두 선의 차이.',
    params: [period('fast', '단기', 12), period('slow', '장기', 26), period('signal', '시그널', 9)],
    compute: (b, p) => {
      const r = calc.macd(b.close, p.fast, p.slow, p.signal)
      return {
        plots: [
          {
            id: 'hist',
            label: 'Hist',
            kind: 'histogram',
            values: r.hist,
            color: UP,
            colors: r.hist.map((h) => (h === null ? null : h >= 0 ? 'rgba(229,72,77,0.6)' : 'rgba(62,99,221,0.6)')),
          },
          { id: 'macd', label: 'MACD', kind: 'line', values: r.macd, color: '#42a5f5' },
          { id: 'signal', label: 'Signal', kind: 'line', values: r.signal, color: '#ff9800' },
        ],
        levels: [{ value: 0 }],
        format: 'price', // MACD is in price units (KRW → integers)
      }
    },
  },
  {
    id: 'rsi',
    name: 'RSI',
    short: 'RSI',
    group: 'pane',
    description:
      '일정 기간 상승폭과 하락폭의 비율(0~100, Wilder 방식). 70 이상은 과매수, 30 이하는 과매도로 보며, 가격과 RSI가 반대로 움직이는 다이버전스는 추세 전환 신호입니다.',
    params: [period('n', '기간', 14)],
    compute: (b, p) => ({
      plots: [{ id: 'rsi', label: '', kind: 'line', values: calc.rsi(b.close, p.n), color: '#ce93d8' }],
      levels: [{ value: 70 }, { value: 30 }],
      format: 'number',
    }),
  },
  {
    id: 'stoch',
    name: '스토캐스틱 슬로우',
    short: 'Stoch',
    group: 'pane',
    description:
      '최근 n기간 고저 범위에서 현재가의 위치(0~100)를 평활화한 지표. 80 이상 과매수, 20 이하 과매도이며 %K가 %D를 상향 돌파하면 매수 신호입니다.',
    params: [period('n', '기간', 14), period('k', '%K', 3), period('d', '%D', 3)],
    compute: (b, p) => {
      const r = calc.stochasticSlow(b, p.n, p.k, p.d)
      return {
        plots: [
          { id: 'k', label: '%K', kind: 'line', values: r.k, color: '#42a5f5' },
          { id: 'd', label: '%D', kind: 'line', values: r.d, color: '#ff9800' },
        ],
        levels: [{ value: 80 }, { value: 20 }],
        format: 'number',
      }
    },
  },
  {
    id: 'dmi',
    name: 'DMI / ADX',
    short: 'DMI',
    group: 'pane',
    description:
      '+DI가 -DI 위면 상승 추세, 아래면 하락 추세. ADX는 방향과 무관한 추세의 강도로 25 이상이면 추세가 뚜렷하고 20 미만이면 횡보로 봅니다.',
    params: [period('n', 'DI 기간', 14), period('adx', 'ADX 기간', 14)],
    compute: (b, p) => {
      const r = calc.dmi(b, p.n, p.adx)
      return {
        plots: [
          { id: 'plus', label: '+DI', kind: 'line', values: r.plusDI, color: UP },
          { id: 'minus', label: '-DI', kind: 'line', values: r.minusDI, color: '#4f7cff' },
          { id: 'adx', label: 'ADX', kind: 'line', values: r.adx, color: '#e0e0e0', lineWidth: 2 },
        ],
        levels: [{ value: 25 }],
        format: 'number',
      }
    },
  },
  {
    id: 'cci',
    name: 'CCI',
    short: 'CCI',
    group: 'pane',
    description: '평균가격이 이동평균에서 얼마나 벗어났는지 측정. +100 이상은 과매수·강세, -100 이하는 과매도·약세로 봅니다.',
    params: [period('n', '기간', 20)],
    compute: (b, p) => ({
      plots: [{ id: 'cci', label: '', kind: 'line', values: calc.cci(b, p.n), color: '#4dd0e1' }],
      levels: [{ value: 100 }, { value: -100 }],
      format: 'number',
    }),
  },
  {
    id: 'williams',
    name: 'Williams %R',
    short: '%R',
    group: 'pane',
    description: '최근 고점 대비 현재가의 위치(0~-100). -20 이상은 과매수, -80 이하는 과매도입니다.',
    params: [period('n', '기간', 14)],
    compute: (b, p) => ({
      plots: [{ id: 'wr', label: '', kind: 'line', values: calc.williamsR(b, p.n), color: '#aed581' }],
      levels: [{ value: -20 }, { value: -80 }],
      format: 'number',
    }),
  },
  {
    id: 'mfi',
    name: 'MFI',
    short: 'MFI',
    group: 'pane',
    description: '거래량을 반영한 RSI(자금흐름지수). 80 이상은 과매수, 20 이하는 과매도로 봅니다.',
    params: [period('n', '기간', 14)],
    compute: (b, p) => ({
      plots: [{ id: 'mfi', label: '', kind: 'line', values: calc.mfi(b, p.n), color: '#ffb74d' }],
      levels: [{ value: 80 }, { value: 20 }],
      format: 'number',
    }),
  },
  {
    id: 'atr',
    name: 'ATR',
    short: 'ATR',
    group: 'pane',
    description: '평균 실제 변동폭. 방향이 아닌 변동성의 크기를 보여주며, 손절폭(예: 2×ATR)이나 포지션 크기를 정할 때 씁니다.',
    params: [period('n', '기간', 14)],
    compute: (b, p) => ({
      plots: [{ id: 'atr', label: '', kind: 'line', values: calc.atr(b, p.n), color: '#90a4ae' }],
      format: 'price',
    }),
  },
  {
    id: 'obv',
    name: 'OBV',
    short: 'OBV',
    group: 'pane',
    description: '상승일 거래량은 더하고 하락일 거래량은 빼서 누적한 값. 가격보다 먼저 움직이는 경우가 많아 수급(매집·분산) 판단에 씁니다.',
    params: [],
    compute: (b) => ({
      plots: [{ id: 'obv', label: '', kind: 'line', values: calc.obv(b), color: '#4fc3f7' }],
      format: 'volume',
    }),
  },
  {
    id: 'disparity',
    name: '이격도',
    short: '이격도',
    group: 'pane',
    description: '종가 ÷ 이동평균 × 100. 100에서 크게 벗어날수록 이동평균으로 되돌아가려는 경향이 있어 과열·침체 판단에 씁니다.',
    params: [period('n', '기간', 20)],
    compute: (b, p) => ({
      plots: [{ id: 'disp', label: '', kind: 'line', values: calc.disparity(b.close, p.n), color: '#f48fb1' }],
      levels: [{ value: 100 }],
      format: 'number',
    }),
  },
  {
    id: 'psy',
    name: '투자심리선',
    short: '심리선',
    group: 'pane',
    description: '최근 n기간 중 상승 마감한 비율(%). 75% 이상은 과열(매도 고려), 25% 이하는 침체(매수 고려)로 봅니다.',
    params: [period('n', '기간', 10, 100)],
    compute: (b, p) => ({
      plots: [{ id: 'psy', label: '', kind: 'line', values: calc.psychologicalLine(b.close, p.n), color: '#fff176' }],
      levels: [{ value: 75 }, { value: 25 }],
      format: 'number',
    }),
  },
]

export const INDICATOR_MAP: Record<string, IndicatorDef> = Object.fromEntries(INDICATORS.map((d) => [d.id, d]))

export const defaultParams = (def: IndicatorDef) => Object.fromEntries(def.params.map((p) => [p.key, p.default]))

export const DEFAULT_INDICATORS: IndicatorConfig[] = [
  { id: 'ma', params: defaultParams(INDICATOR_MAP.ma) },
  { id: 'volume', params: defaultParams(INDICATOR_MAP.volume) },
]

/** Keep only known indicators; fill missing/invalid params with defaults (settings may be stale). */
export function sanitize(configs: unknown): IndicatorConfig[] {
  if (!Array.isArray(configs)) return DEFAULT_INDICATORS
  const seen = new Set<string>()
  const out: IndicatorConfig[] = []
  for (const c of configs) {
    const def = c && typeof c === 'object' ? INDICATOR_MAP[(c as IndicatorConfig).id] : undefined
    if (!def || seen.has(def.id)) continue
    seen.add(def.id)
    const raw = (c as IndicatorConfig).params ?? {}
    const params: Record<string, number> = {}
    for (const p of def.params) {
      const v = Number(raw[p.key])
      params[p.key] = Number.isFinite(v) && v >= p.min && v <= p.max ? v : p.default
    }
    out.push({ id: def.id, params })
  }
  return out
}
