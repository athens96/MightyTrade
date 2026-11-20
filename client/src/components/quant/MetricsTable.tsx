import type { Market } from '../../api/types'
import type { Metrics } from '../../api/quant'
import { priceFormatter } from '../chart/format'

type Kind = 'pct' | 'pctAbs' | 'num' | 'days' | 'money' | 'count' | 'bars'

const ROWS: { key: keyof Metrics; label: string; kind: Kind; help: string; tradeOnly?: boolean }[] = [
  { key: 'total_return_pct', label: '총 수익률', kind: 'pct', help: '기간 전체 수익률 (비용 차감 후)' },
  { key: 'cagr_pct', label: '연평균 수익률 (CAGR)', kind: 'pct', help: '복리로 환산한 1년 평균 수익률' },
  { key: 'mdd_pct', label: '최대 낙폭 (MDD)', kind: 'pct', help: '고점 대비 가장 크게 떨어진 비율. 실제로 견뎌야 하는 최악의 손실' },
  { key: 'max_dd_days', label: '최장 낙폭 기간', kind: 'days', help: '고점을 회복하기까지 걸린 가장 긴 기간' },
  { key: 'volatility_pct', label: '연 변동성', kind: 'pctAbs', help: '일간 수익률 표준편차의 연환산' },
  { key: 'sharpe', label: '샤프 지수', kind: 'num', help: '변동성 대비 수익 (무위험수익률 0 가정). 1 이상이면 양호' },
  { key: 'sortino', label: '소르티노 지수', kind: 'num', help: '하락 변동성만 고려한 위험 대비 수익' },
  { key: 'calmar', label: '칼마 지수', kind: 'num', help: 'CAGR ÷ |MDD|. 낙폭 대비 수익' },
  { key: 'final_equity', label: '최종 자산', kind: 'money', help: '마지막 날 평가액 (보유 중이면 종가 평가)' },
  { key: 'trades', label: '매매 횟수', kind: 'count', help: '청산 완료된 매매 수', tradeOnly: true },
  { key: 'win_rate_pct', label: '승률', kind: 'pctAbs', help: '수익으로 끝난 매매 비율', tradeOnly: true },
  { key: 'avg_return_pct', label: '평균 매매 수익률', kind: 'pct', help: '매매 1회당 평균 수익률', tradeOnly: true },
  { key: 'avg_win_pct', label: '평균 이익', kind: 'pct', help: '이익 매매의 평균 수익률', tradeOnly: true },
  { key: 'avg_loss_pct', label: '평균 손실', kind: 'pct', help: '손실 매매의 평균 수익률', tradeOnly: true },
  { key: 'profit_factor', label: '손익비 (Profit Factor)', kind: 'num', help: '총이익 ÷ 총손실. 1보다 커야 수익', tradeOnly: true },
  { key: 'best_pct', label: '최고 매매', kind: 'pct', help: '가장 좋았던 매매 수익률', tradeOnly: true },
  { key: 'worst_pct', label: '최악 매매', kind: 'pct', help: '가장 나빴던 매매 수익률', tradeOnly: true },
  { key: 'avg_bars', label: '평균 보유 기간', kind: 'bars', help: '매매당 평균 보유 거래일', tradeOnly: true },
  { key: 'exposure_pct', label: '시장 노출', kind: 'pctAbs', help: '전체 기간 중 주식을 보유한 비율', tradeOnly: true },
]

export default function MetricsTable({ strategy, benchmark, market }: { strategy: Metrics; benchmark: Metrics; market: Market }) {
  const money = priceFormatter(market)
  const fmt = (v: number | null | undefined, kind: Kind) => {
    if (v === null || v === undefined) return '-'
    switch (kind) {
      case 'pct':
        return `${v > 0 ? '+' : ''}${v.toFixed(2)}%`
      case 'pctAbs':
        return `${v.toFixed(1)}%`
      case 'num':
        return v.toFixed(2)
      case 'days':
        return `${v.toLocaleString()}일`
      case 'bars':
        return `${v.toFixed(1)}일`
      case 'money':
        return money(v)
      case 'count':
        return v.toLocaleString()
    }
  }
  const cls = (v: number | null | undefined, kind: Kind) => (kind === 'pct' && v ? (v > 0 ? 'up' : 'down') : '')
  return (
    <table className="metrics">
      <thead>
        <tr>
          <th />
          <th>전략</th>
          <th>단순 보유</th>
        </tr>
      </thead>
      <tbody>
        {ROWS.map((r) => (
          <tr key={r.key} title={r.help}>
            <td>{r.label}</td>
            <td className={cls(strategy[r.key], r.kind)}>{fmt(strategy[r.key], r.kind)}</td>
            <td className={r.tradeOnly ? 'muted' : cls(benchmark[r.key], r.kind)}>{r.tradeOnly ? '' : fmt(benchmark[r.key], r.kind)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
