import { useState } from 'react'
import type { AutotradeOverview, BotCategory, BotMode } from '../../api/autotrade'
import { MOBILE, useMediaQuery } from '../../hooks/useMediaQuery'

const CAT_LABEL: Record<BotCategory, string> = {
  swing: '스윙',
  daytrade: '단타',
  scalp: '스캘핑',
  rebalance: '리밸런싱',
  rotation: '순환매',
  dca: '적립식',
  webhook: '웹훅',
}
const MODE_LABEL: Record<BotMode, string> = {
  paper: '모의',
  semi: '승인 후 주문',
  live: '실거래',
}

const fmtKrw = (v: number) => `${Math.round(v).toLocaleString()}원`
const fmtPct = (v: number | null | undefined) => (v == null ? '-' : `${v > 0 ? '+' : ''}${v.toFixed(2)}%`)
const cls = (v: number | null | undefined) => (!v ? '' : v > 0 ? 'up' : 'down')

function CombinedReturnChart({ points }: { points: AutotradeOverview['combined_equity'] }) {
  const [hover, setHover] = useState<number | null>(null)
  if (points.length < 2) {
    return <p className="muted small">봇 실행 기록이 2일 이상 쌓이면 전체 합산 수익률 곡선이 표시됩니다.</p>
  }
  const W = 640
  const H = 120
  const P = 8
  const vals: number[] = [0]
  for (const pt of points) {
    if (pt.paper_return_pct != null) vals.push(pt.paper_return_pct)
    if (pt.live_return_pct != null) vals.push(pt.live_return_pct)
  }
  const lo = Math.min(...vals)
  const hi = Math.max(...vals)
  const x = (i: number) => P + (i / (points.length - 1)) * (W - 2 * P)
  const y = (v: number) => (hi === lo ? H / 2 : H - P - ((v - lo) / (hi - lo)) * (H - 2 * P))

  const buildPath = (key: 'paper_return_pct' | 'live_return_pct') => {
    let started = false
    return points
      .map((p, i) => {
        const val = p[key]
        if (val == null) return ''
        const cmd = started ? 'L' : 'M'
        started = true
        return `${cmd}${x(i).toFixed(1)},${y(val).toFixed(1)}`
      })
      .join('')
  }

  const paperPath = buildPath('paper_return_pct')
  const livePath = buildPath('live_return_pct')
  const h = Math.max(0, Math.min(points.length - 1, hover ?? points.length - 1))
  const cur = points[h]

  return (
    <div className="svg-chart">
      <div className="small muted" style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
        <span>합산 수익률 추이 · <b>{cur.date}</b></span>
        {cur.live_return_pct != null && (
          <span><i className="dot on" style={{ background: '#3fb950' }} /> 실전 합산 <b className={cls(cur.live_return_pct)}>{fmtPct(cur.live_return_pct)}</b></span>
        )}
        {cur.paper_return_pct != null && (
          <span><i className="dot on" style={{ background: '#4fc3f7' }} /> 모의 합산 <b className={cls(cur.paper_return_pct)}>{fmtPct(cur.paper_return_pct)}</b></span>
        )}
      </div>
      <svg
        className="spark"
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="none"
        role="img"
        aria-label="전체 봇 합산 수익률 곡선"
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect()
          setHover(Math.round((((e.clientX - r.left) / r.width) * W - P) / ((W - 2 * P) / (points.length - 1))))
        }}
        onMouseLeave={() => setHover(null)}
      >
        <line x1={P} x2={W - P} y1={y(0)} y2={y(0)} className="axis-zero" />
        {paperPath && <path d={paperPath} fill="none" stroke="#4fc3f7" strokeWidth={2} strokeDasharray="4 2" />}
        {livePath && <path d={livePath} fill="none" stroke="#3fb950" strokeWidth={2.2} />}
      </svg>
    </div>
  )
}

interface Props {
  overview: AutotradeOverview
  onSelectBot: (botId: string) => void
}

export default function OverviewPanel({ overview, onSelectBot }: Props) {
  const isMobile = useMediaQuery(MOBILE)
  const [open, setOpen] = useState(() => typeof window === 'undefined' || !window.matchMedia(MOBILE).matches)
  const [tab, setTab] = useState<'summary' | 'scorecard' | 'schedule'>('summary')

  const { paper, live, execution, duplicate_symbols: dupes } = overview
  const slip = execution.avg_slippage_bps

  return (
    <section className="panel" style={{ marginBottom: 12 }}>
      <div className="account-head" style={{ justifyContent: 'space-between' }}>
        <h3 style={{ margin: 0, fontSize: 15 }}>📊 통합 관제 · 체결 성적표</h3>
        {!isMobile && (
          <span className="muted small">
            전체 {overview.total_bots}개 중 <b>{overview.enabled_bots}개 가동</b>
            {overview.pending_approvals_count > 0 && <> · <b className="up">✋ 승인 대기 {overview.pending_approvals_count}건</b></>}
            {overview.tripped_count > 0 && <> · <b className="down">🛑 보호 잠금 {overview.tripped_count}건</b></>}
            {dupes.length > 0 && <> · <b>⚠️ 중복 종목 {dupes.length}건</b></>}
          </span>
        )}
        <div className="spacer" />
        {open && !isMobile && (
          <div className="tabs">
            <button
              type="button"
              className={tab === 'summary' ? 'active' : ''}
              title="모의·실거래 분리 합산 자산과 전체 수익률 추이, 중복 종목 경고를 한눈에 봅니다"
              onClick={() => setTab('summary')}
            >
              통합 요약
            </button>
            <button
              type="button"
              className={tab === 'scorecard' ? 'active' : ''}
              title="봇별 모의 수익률 대비 실거래 수익률 격차와 실제 체결 슬리피지(bps) 성적표를 확인합니다"
              onClick={() => setTab('scorecard')}
            >
              체결 성적표 ({execution.total_fills}건)
            </button>
            <button
              type="button"
              className={tab === 'schedule' ? 'active' : ''}
              title="모든 봇의 다음 자동 실행 예정 시각·주기와 최근 실행일을 확인합니다"
              onClick={() => setTab('schedule')}
            >
              실행 스케줄
            </button>
          </div>
        )}
        <button
          type="button"
          className="small"
          title={open ? '통합 관제 패널을 접어 개별 봇 화면을 넓게 봅니다' : '전체 봇 합산 자산과 체결 성적표 패널을 펼칩니다'}
          onClick={() => setOpen((v) => !v)}
        >
          {open ? '접기 ▲' : '펼치기 ▼'}
        </button>
      </div>
      {isMobile && (
        <div className="muted small" style={{ marginTop: 4 }}>
          전체 {overview.total_bots}개 중 <b>{overview.enabled_bots}개 가동</b>
          {overview.pending_approvals_count > 0 && <> · <b className="up">✋ 승인 대기 {overview.pending_approvals_count}건</b></>}
          {overview.tripped_count > 0 && <> · <b className="down">🛑 보호 잠금 {overview.tripped_count}건</b></>}
          {dupes.length > 0 && <> · <b>⚠️ 중복 종목 {dupes.length}건</b></>}
        </div>
      )}
      {open && isMobile && (
        <div className="tabs" style={{ marginTop: 8 }}>
          <button
            type="button"
            className={tab === 'summary' ? 'active' : ''}
            title="모의·실거래 분리 합산 자산과 전체 수익률 추이, 중복 종목 경고를 한눈에 봅니다"
            onClick={() => setTab('summary')}
          >
            통합 요약
          </button>
          <button
            type="button"
            className={tab === 'scorecard' ? 'active' : ''}
            title="봇별 모의 수익률 대비 실거래 수익률 격차와 실제 체결 슬리피지(bps) 성적표를 확인합니다"
            onClick={() => setTab('scorecard')}
          >
            체결 성적표 ({execution.total_fills}건)
          </button>
          <button
            type="button"
            className={tab === 'schedule' ? 'active' : ''}
            title="모든 봇의 다음 자동 실행 예정 시각·주기와 최근 실행일을 확인합니다"
            onClick={() => setTab('schedule')}
          >
            실행 스케줄
          </button>
        </div>
      )}

      {open && (
        <>
          <div className="stat-tiles" style={{ marginTop: 10 }}>
            <div className="stat-tile" title={`반자동·실거래 봇 합산 (미국 달러는 기준환율 ${overview.usd_krw_ref.toLocaleString()}원으로 환산 합계)`}>
              <span className="muted small">실전 운용 합산 (반자동·실거래 {live.enabled}/{live.bots})</span>
              <b className={cls(live.return_pct)}>{live.bots ? `${fmtKrw(live.equity_krw)} (${fmtPct(live.return_pct)})` : '운용 봇 없음'}</b>
              <span className="small muted">
                투입 중 {fmtKrw(live.invested_krw)} · 비중 국내 {live.kr_share_pct}% / 미국 {live.us_share_pct}%
              </span>
            </div>
            <div className="stat-tile" title="가상 자금으로 전략을 검증 중인 모의 봇들의 합산 평가액과 수익률입니다">
              <span className="muted small">모의 테스트 합산 ({paper.enabled}/{paper.bots})</span>
              <b className={cls(paper.return_pct)}>{paper.bots ? `${fmtKrw(paper.equity_krw)} (${fmtPct(paper.return_pct)})` : '모의 봇 없음'}</b>
              <span className="small muted">
                원금 {fmtKrw(paper.capital_krw)} · 손익 {paper.pnl_krw >= 0 ? '+' : ''}{fmtKrw(paper.pnl_krw)}
              </span>
            </div>
            <div className="stat-tile" title="신호 발생 당시 가격과 실제 토스증권 체결가의 평균 차이입니다 (1bp = 0.01%, 음수면 더 싸게 사고 비싸게 판 유리한 체결)">
              <span className="muted small">실전 체결 슬리피지 품질</span>
              <b className={slip == null ? '' : slip <= 5 ? 'up' : 'down'}>
                {slip == null ? '실체결 대기' : `${slip > 0 ? '+' : ''}${slip.toFixed(1)} bps`}
              </b>
              <span className="small muted">
                누적 실체결 {execution.total_fills}건 · {slip == null ? '체결 후 자동 집계' : slip <= 0 ? '신호가 대비 유리 체결' : `평균 ${(slip / 100).toFixed(3)}% 비용`}
              </span>
            </div>
            <div className="stat-tile" title="즉시 확인이 필요한 승인 대기 주문·보호 잠금·중복 종목 상태입니다">
              <span className="muted small">관제 점검 항목</span>
              <b>
                승인대기 {overview.pending_approvals_count} · 잠금 {overview.tripped_count} · 중복 {dupes.length}
              </b>
              <span className="small muted">모의 자산과 실전 계좌 원장은 완벽히 분리 집계됩니다</span>
            </div>
          </div>

          {dupes.length > 0 && (
            <div className="warn small" style={{ marginTop: 8 }}>
              <b>⚠️ 여러 봇이 같은 종목을 함께 다루고 있습니다:</b>{' '}
              {dupes.map((d, idx) => (
                <span key={`${d.market}:${d.symbol}`}>
                  {idx > 0 && ' / '}
                  <b>{d.symbol}</b>({d.market}){d.live_conflict ? ' [실전 중복 주의]' : ''}:{' '}
                  {d.bots.map((b, j) => (
                    <button
                      key={b.id}
                      type="button"
                      className="link small"
                      title={`${b.name} 봇 상세 화면으로 이동합니다`}
                      onClick={() => onSelectBot(b.id)}
                    >
                      {j > 0 ? `, ${b.name}` : b.name}
                    </button>
                  ))}
                </span>
              ))}
            </div>
          )}

          {tab === 'summary' && <CombinedReturnChart points={overview.combined_equity} />}

          {tab === 'scorecard' && (
            <div className="trades-wrap" style={{ marginTop: 8 }}>
              <table className="trades left-first">
                <thead>
                  <tr>
                    <th>봇 이름</th>
                    <th>현재 모드</th>
                    <th>모의 수익률 (매매/승률)</th>
                    <th>실전 수익률 (매매/승률)</th>
                    <th>실전-모의 격차</th>
                    <th>평균 체결 슬리피지</th>
                  </tr>
                </thead>
                <tbody>
                  {execution.items.map((it) => (
                    <tr key={it.bot_id}>
                      <td>
                        <button
                          type="button"
                          className="link"
                          title={`${it.name} 상세 보기`}
                          onClick={() => onSelectBot(it.bot_id)}
                        >
                          <b>{it.name}</b>
                        </button>{' '}
                        <span className={`cat-badge ${it.category}`}>{CAT_LABEL[it.category]}</span>
                      </td>
                      <td>
                        <span className={`mode-badge ${it.mode}`}>{MODE_LABEL[it.mode]}</span>
                      </td>
                      <td className={cls(it.paper_return_pct)}>
                        {fmtPct(it.paper_return_pct)}{' '}
                        <span className="muted small">
                          ({it.paper_trades}회{it.paper_win_rate_pct != null ? ` · 승률 ${it.paper_win_rate_pct}%` : ''})
                        </span>
                      </td>
                      <td className={cls(it.live_return_pct)}>
                        {fmtPct(it.live_return_pct)}{' '}
                        <span className="muted small">
                          ({it.live_trades}회{it.live_win_rate_pct != null ? ` · 승률 ${it.live_win_rate_pct}%` : ''})
                        </span>
                      </td>
                      <td className={cls(it.return_gap_pct)}>
                        {it.return_gap_pct != null ? `${it.return_gap_pct > 0 ? '+' : ''}${it.return_gap_pct.toFixed(2)}%p` : '-'}
                      </td>
                      <td className={it.avg_slippage_bps == null ? 'muted' : it.avg_slippage_bps <= 5 ? 'up' : 'down'}>
                        {it.avg_slippage_bps == null
                          ? '-'
                          : `${it.avg_slippage_bps > 0 ? '+' : ''}${it.avg_slippage_bps.toFixed(1)} bps (${it.fills_count}건)`}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {tab === 'schedule' && (
            <div className="trades-wrap" style={{ marginTop: 8 }}>
              <table className="trades left-first">
                <thead>
                  <tr>
                    <th>봇 이름</th>
                    <th>상태</th>
                    <th>실행 스케줄 규칙</th>
                    <th>최근 실행일</th>
                    <th>알림 상태</th>
                  </tr>
                </thead>
                <tbody>
                  {overview.upcoming.map((u) => (
                    <tr key={u.bot_id}>
                      <td>
                        <button
                          type="button"
                          className="link"
                          title={`${u.name} 상세 보기`}
                          onClick={() => onSelectBot(u.bot_id)}
                        >
                          <b>{u.name}</b>
                        </button>{' '}
                        <span className="muted small">({u.market})</span>
                      </td>
                      <td>
                        <span className={`dot ${u.enabled ? 'on' : ''}`} /> {u.enabled ? '가동 중' : '대기'} · {MODE_LABEL[u.mode]}
                      </td>
                      <td className="small">{u.schedule}</td>
                      <td>{u.last_run_date ?? '-'}</td>
                      <td className="small">
                        {u.pending_approval_orders > 0
                          ? <span className="up">✋ 승인 대기 {u.pending_approval_orders}건</span>
                          : u.tripped
                            ? <span className="down">🛑 서킷브레이커 잠금</span>
                            : <span className="muted">정상</span>}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </section>
  )
}
