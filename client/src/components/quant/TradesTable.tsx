import type { Interval, Market } from '../../api/types'
import type { BacktestTrade } from '../../api/quant'
import { priceFormatter } from '../chart/format'

const fmtDay = (t: number | null) => (t ? new Date(t * 1000).toISOString().slice(0, 10) : '-')
const fmtTime = (t: number | null) => {
  if (!t) return '-'
  const d = new Date(t * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

export default function TradesTable({ trades, market, interval = '1d' }: { trades: BacktestTrade[]; market: Market; interval?: Interval }) {
  const money = priceFormatter(market)
  const intraday = interval !== '1d' && interval !== '1w'
  const fmtDate = intraday ? fmtTime : fmtDay
  const unit = intraday ? '봉' : '일'

  return (
    <>
      <h4>매매 내역 ({trades.length})</h4>
      <div className="trades-wrap">
        <table className="trades">
          <thead>
            <tr>
              <th>#</th>
              <th>{intraday ? '진입 시각' : '진입일'}</th>
              <th>진입가</th>
              <th>{intraday ? '청산 시각' : '청산일'}</th>
              <th>청산가</th>
              <th>수량</th>
              <th>손익</th>
              <th>수익률</th>
              <th>보유</th>
              <th>사유</th>
            </tr>
          </thead>
          <tbody>
            {[...trades].reverse().map((t, i) => (
              <tr key={`${t.entry_time}-${i}`}>
                <td className="muted">{trades.length - i}</td>
                <td>{fmtDate(t.entry_time)}</td>
                <td>{money(t.entry_price)}</td>
                <td>{t.open ? <span className="muted">보유 중</span> : fmtDate(t.exit_time)}</td>
                <td>{t.exit_price !== null ? money(t.exit_price) : '-'}</td>
                <td>{t.qty.toLocaleString()}</td>
                <td className={(t.pnl ?? 0) > 0 ? 'up' : 'down'}>{t.pnl !== null ? money(t.pnl) : '-'}</td>
                <td className={(t.return_pct ?? 0) > 0 ? 'up' : 'down'}>
                  {t.return_pct !== null ? `${t.return_pct > 0 ? '+' : ''}${t.return_pct.toFixed(2)}%` : '-'}
                </td>
                <td>{t.bars}{unit}</td>
                <td className="muted small">
                  {t.entry_reason}
                  {t.exit_reason && ` → ${t.exit_reason}`}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  )
}
