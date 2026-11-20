import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { autotradeApi, PREFILL_KEY, type Bot, type BotEvent, type BotInput, type Evaluation } from '../api/autotrade'
import { ApiError } from '../api/client'
import { quantApi, type StrategyMeta } from '../api/quant'
import { priceFormatter } from '../components/chart/format'
import BotForm from '../components/autotrade/BotForm'

const money = (b: Bot, v: number | null | undefined) =>
  v == null ? '-' : `${priceFormatter(b.market)(v)}${b.market === 'KR' ? '원' : ' USD'}`
const pct = (v: number | null | undefined) => (v == null ? '-' : `${v > 0 ? '+' : ''}${v.toFixed(2)}%`)
const cls = (v: number | null | undefined) => (!v ? '' : v > 0 ? 'up' : 'down')

function botPnl(b: Bot): number | null {
  const s = b.state
  if (b.mode === 'paper') return s.equity_now != null ? (s.equity_now / b.capital - 1) * 100 : null
  const last = s.equity[s.equity.length - 1]
  return last ? (last.value / b.capital - 1) * 100 : null
}

/** Equity over trading days (single series: the title names it). Hover shows the value. */
function Sparkline({ points, bot }: { points: { date: string; value: number }[]; bot: Bot }) {
  const [hover, setHover] = useState<number | null>(null)
  if (points.length < 2) return <p className="muted small">실행 기록이 쌓이면 자산 추이가 표시됩니다.</p>
  const W = 600, H = 120, P = 6
  const vals = points.map((p) => p.value)
  const lo = Math.min(...vals), hi = Math.max(...vals)
  const x = (i: number) => P + (i / (points.length - 1)) * (W - 2 * P)
  const y = (v: number) => (hi === lo ? H / 2 : H - P - ((v - lo) / (hi - lo)) * (H - 2 * P))
  const d = points.map((p, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(p.value).toFixed(1)}`).join('')
  const h = hover ?? points.length - 1
  return (
    <div className="svg-chart">
      <div className="small muted">자산 추이 · {points[h].date} <b className="plain">{money(bot, points[h].value)}</b></div>
      <svg className="spark" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img" aria-label="봇 자산 추이"
           onMouseMove={(e) => {
             const r = (e.currentTarget as SVGSVGElement).getBoundingClientRect()
             setHover(Math.round((((e.clientX - r.left) / r.width) * W - P) / ((W - 2 * P) / (points.length - 1))))
           }}
           onMouseLeave={() => setHover(null)}>
        <line x1={P} x2={W - P} y1={y(bot.capital)} y2={y(bot.capital)} className="axis-zero" />
        <path d={d} fill="none" stroke="#4fc3f7" strokeWidth={2} />
        <circle cx={x(h)} cy={y(points[h].value)} r={4} fill="#4fc3f7" stroke="#111418" strokeWidth={2} />
      </svg>
    </div>
  )
}

function BotDetail({ bot, strategies, onChanged, onDeleted }: {
  bot: Bot; strategies: StrategyMeta[]; onChanged: (b: Bot) => void; onDeleted: () => void
}) {
  const [events, setEvents] = useState<BotEvent[]>([])
  const [evaluation, setEvaluation] = useState<Evaluation | null>(null)
  const [otp, setOtp] = useState('')
  const [confirmLive, setConfirmLive] = useState(false)
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const meta = strategies.find((s) => s.id === bot.strategy)

  const loadEvents = useCallback(() => {
    autotradeApi.bot(bot.id).then((r) => setEvents(r.events)).catch(() => undefined)
  }, [bot.id])
  useEffect(() => {
    setEvaluation(null)
    setError('')
    setConfirmLive(false)
    loadEvents()
  }, [bot.id, loadEvents])

  const act = async (label: string, fn: () => Promise<{ bot?: Bot } | void>) => {
    setBusy(label)
    setError('')
    try {
      const r = await fn()
      if (r && 'bot' in r && r.bot) onChanged(r.bot)
      loadEvents()
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy('')
      setOtp('')
    }
  }

  const s = bot.state
  const pos = s.position
  const unreal = pos.qty && s.last_price ? (s.last_price / pos.entry_price - 1) * 100 : null

  return (
    <section className="bot-detail">
      <div className="account-head">
        <h3>{bot.name}</h3>
        <span className={`mode-badge ${bot.mode}`}>{bot.mode === 'paper' ? '모의' : '실거래'}</span>
        <span className={`state-badge ${bot.enabled ? 'on' : ''}`}>{bot.enabled ? '켜짐' : '꺼짐'}</span>
        <div className="spacer" />
        <button onClick={() => act('toggle', () => (bot.enabled ? autotradeApi.disable(bot.id) : autotradeApi.enable(bot.id)))} disabled={!!busy}
                className={bot.enabled ? '' : bot.mode === 'live' ? 'danger' : 'primary'}>
          {bot.enabled ? '끄기' : bot.mode === 'live' ? '켜기 (실제 주문)' : '켜기'}
        </button>
      </div>
      <p className="muted small">
        {bot.symbol} · {meta?.name ?? bot.strategy} ({Object.entries(bot.params).map(([k, v]) => `${k}=${v}`).join(', ')}) · 운용 자금 {money(bot, bot.capital)}
        {' · '}매일 정규장 시작 1분 뒤 전날 종가 기준으로 판단·실행
      </p>
      {error && <p className="error">{error}</p>}

      <div className="stat-tiles">
        <div className="stat-tile">
          <span className="muted small">보유</span>
          <b>{pos.qty ? `${pos.qty.toLocaleString()}주` : '없음'}</b>
          {pos.qty > 0 && <span className="small">평균 {money(bot, pos.entry_price)} · <span className={cls(unreal)}>{pct(unreal)}</span></span>}
        </div>
        <div className="stat-tile">
          <span className="muted small">{bot.mode === 'paper' ? '모의 평가자산' : '봇 손익 (자금 대비)'}</span>
          <b className={cls(botPnl(bot))}>{pct(botPnl(bot))}</b>
          {bot.mode === 'paper' && <span className="small">{money(bot, s.equity_now)} · 현금 {money(bot, s.cash)}</span>}
        </div>
        <div className="stat-tile">
          <span className="muted small">마지막 실행</span>
          <b>{s.last_run_date ?? '-'}</b>
          <span className="small muted">대기 주문 {s.pending.length}건</span>
        </div>
        <div className="stat-tile">
          <span className="muted small">매매</span>
          <b>{s.trades.filter((t) => t.exit_date).length}회</b>
          <span className="small muted">승 {s.trades.filter((t) => (t.pnl ?? 0) > 0).length}</span>
        </div>
      </div>

      <div className="bot-actions">
        <button disabled={!!busy} onClick={() => act('eval', async () => setEvaluation(await autotradeApi.evaluate(bot.id)))}>
          {busy === 'eval' ? '점검 중…' : '지금 점검 (주문 없음)'}
        </button>
        {bot.mode === 'paper' && (
          <>
            <button disabled={!!busy} onClick={() => act('run', () => autotradeApi.run(bot.id))}>{busy === 'run' ? '실행 중…' : '오늘 실행 (모의)'}</button>
            <button disabled={!!busy} onClick={() => confirm('모의 기록을 모두 지울까요?') && act('reset', () => autotradeApi.reset(bot.id))}>모의 초기화</button>
          </>
        )}
        <button disabled={!!busy} onClick={() => setEditing(true)}>설정 변경</button>
        <button className="link danger-link" disabled={!!busy}
                onClick={() => confirm('봇을 삭제할까요?') && act('del', async () => { await autotradeApi.remove(bot.id); onDeleted() })}>삭제</button>
      </div>

      {evaluation && (
        <div className="panel">
          <h4>점검 결과 · {evaluation.date} 실행 예정 ({evaluation.bar_date} 종가 기준)</h4>
          {evaluation.orders.length ? (
            <ul>{evaluation.orders.map((o, i) => <li key={i} className={o.side === 'buy' ? 'up' : 'down'}>{o.text}</li>)}</ul>
          ) : <p className="muted">신호 없음 — 다음 실행 때 주문하지 않습니다.</p>}
          <p className="muted small">현재 보유 {evaluation.position.qty}주 · 최근가 {money(bot, evaluation.last)}{evaluation.today_open ? ` · 오늘 시가 ${money(bot, evaluation.today_open)}` : ''}</p>
        </div>
      )}

      <div className="panel live-panel">
        {bot.mode === 'paper' ? (
          <>
            <h4>실거래로 전환</h4>
            <p className="small">
              실거래 모드에서 봇을 켜면 <b>실제 주문이 자동으로 나갑니다.</b> 모의 모드로 충분히 확인한 뒤 전환하세요. 주문은 계정의 일일 주문 한도·건당 금액
              한도를 따르고, 상단의 <b>전체 중지</b>로 언제든 멈출 수 있습니다. 전환하면 봇은 꺼진 상태가 됩니다.
            </p>
            <label className="check small"><input type="checkbox" checked={confirmLive} onChange={(e) => setConfirmLive(e.target.checked)} /> 실제 돈으로 자동 주문된다는 것을 이해했습니다</label>
            <div className="modal-actions" style={{ justifyContent: 'flex-start' }}>
              <input className="otp-input" inputMode="numeric" maxLength={6} placeholder="OTP" value={otp} onChange={(e) => setOtp(e.target.value.replace(/\D/g, ''))} />
              <button className="danger" disabled={!confirmLive || otp.length !== 6 || !!busy} onClick={() => act('mode', () => autotradeApi.mode(bot.id, 'live', otp))}>실거래 모드로 전환</button>
            </div>
          </>
        ) : (
          <div className="modal-actions" style={{ justifyContent: 'flex-start' }}>
            <span className="small">실거래 모드입니다. 이미 들어간 주문·보유는 모의로 바꿔도 그대로 남습니다.</span>
            <button disabled={!!busy} onClick={() => act('mode', () => autotradeApi.mode(bot.id, 'paper'))}>모의 모드로 전환</button>
          </div>
        )}
      </div>

      <Sparkline points={s.equity} bot={bot} />

      <h4>매매 ({bot.mode === 'paper' ? '모의' : '실거래'})</h4>
      <div className="trades-wrap">
        <table className="trades">
          <thead><tr><th>진입</th><th>진입가</th><th>수량</th><th>청산</th><th>청산가</th><th>손익</th><th>수익률</th><th>사유</th></tr></thead>
          <tbody>
            {[...s.trades].reverse().map((t, i) => (
              <tr key={i}>
                <td>{t.entry_date}</td><td>{money(bot, t.entry_price)}</td><td>{t.qty.toLocaleString()}</td>
                <td>{t.exit_date ?? <span className="muted">보유 중</span>}</td><td>{t.exit_price != null ? money(bot, t.exit_price) : '-'}</td>
                <td className={cls(t.pnl)}>{t.pnl != null ? money(bot, t.pnl) : '-'}</td><td className={cls(t.return_pct)}>{pct(t.return_pct)}</td>
                <td className="muted small">{t.entry_reason}{t.exit_reason && ` → ${t.exit_reason}`}</td>
              </tr>
            ))}
            {!s.trades.length && <tr><td colSpan={8} className="muted">아직 매매가 없습니다.</td></tr>}
          </tbody>
        </table>
      </div>

      <h4>실행 로그</h4>
      <div className="bot-log">
        {events.map((e) => (
          <div key={e.id} className={`log-row ${e.level}`}>
            <span className="muted">{new Date(e.at).toLocaleString('ko-KR', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hour12: false })}</span>
            <span className={`mode-badge ${e.mode}`}>{e.mode === 'paper' ? '모의' : '실거래'}</span>
            <span>{e.message}</span>
          </div>
        ))}
        {!events.length && <p className="muted small">기록이 없습니다.</p>}
      </div>

      {editing && (
        <BotForm strategies={strategies} bot={bot} initial={bot} onClose={() => setEditing(false)}
                 onSubmit={async (b) => onChanged((await autotradeApi.update(bot.id, b)).bot)} />
      )}
    </section>
  )
}

export default function AutoTradePage() {
  const [bots, setBots] = useState<Bot[]>([])
  const [selected, setSelected] = useState<string | null>(null)
  const [strategies, setStrategies] = useState<StrategyMeta[]>([])
  const [status, setStatus] = useState<{ halted: boolean; scheduler: { running: boolean; last_tick: string | null } } | null>(null)
  const [creating, setCreating] = useState<Partial<BotInput> | null>(null)
  const [resumeOtp, setResumeOtp] = useState('')
  const [error, setError] = useState<{ code: string; message: string } | null>(null)

  const load = useCallback(() => {
    autotradeApi.bots().then((r) => setBots(r.items)).catch((e) => setError({ code: e instanceof ApiError ? e.code : '', message: e.message }))
    autotradeApi.status().then(setStatus).catch(() => undefined)
  }, [])

  useEffect(() => {
    quantApi.strategies().then((r) => setStrategies(r.items)).catch(() => undefined)
    load()
    const prefill = sessionStorage.getItem(PREFILL_KEY) // from Quant Lab: "이 설정으로 봇 만들기"
    if (prefill) {
      sessionStorage.removeItem(PREFILL_KEY)
      setCreating(JSON.parse(prefill))
    }
    const t = window.setInterval(() => document.visibilityState === 'visible' && load(), 30_000)
    return () => window.clearInterval(t)
  }, [load])

  useEffect(() => {
    if (!selected && bots.length) setSelected(bots[0].id)
  }, [bots, selected])

  const upsert = (b: Bot) => setBots((list) => (list.some((x) => x.id === b.id) ? list.map((x) => (x.id === b.id ? b : x)) : [...list, b]))
  const current = bots.find((b) => b.id === selected)

  return (
    <div className="autotrade">
      <div className="account-head autotrade-head">
        <h2>자동매매</h2>
        {status && (
          <span className="small muted">
            <i className={`dot ${status.scheduler.running && !status.halted ? 'on' : ''}`} /> 스케줄러 {status.scheduler.running ? '동작 중' : '정지'}
            {status.scheduler.last_tick && ` · ${new Date(status.scheduler.last_tick).toLocaleTimeString('ko-KR', { hour12: false })}`}
          </span>
        )}
        <div className="spacer" />
        {status?.halted ? (
          <>
            <span className="error small">전체 중지 상태 — 모든 봇이 멈춰 있습니다</span>
            <input className="otp-input" inputMode="numeric" maxLength={6} placeholder="OTP" value={resumeOtp} onChange={(e) => setResumeOtp(e.target.value.replace(/\D/g, ''))} />
            <button disabled={resumeOtp.length !== 6} onClick={async () => {
              try { await autotradeApi.resume(resumeOtp); load() } catch (e) { alert((e as Error).message) } finally { setResumeOtp('') }
            }}>재개</button>
          </>
        ) : (
          <button className="danger" onClick={async () => {
            if (!confirm('모든 봇을 멈추고 봇이 낸 미체결 주문을 취소할까요?')) return
            const r = await autotradeApi.halt()
            alert(`전체 중지됨 · 취소한 주문 ${r.cancelled}건`)
            load()
          }}>■ 전체 중지</button>
        )}
        <button className="primary" onClick={() => setCreating({})}>+ 봇 만들기</button>
      </div>
      {error?.code === 'toss_credentials_required' && <p className="placeholder">토스증권 API 키가 필요합니다. <Link to="/settings">설정에서 등록하기 →</Link></p>}

      <div className="autotrade-body">
        <aside className="bot-list">
          {bots.map((b) => {
            const p = botPnl(b)
            return (
              <button key={b.id} className={`bot-item ${b.id === selected ? 'active' : ''}`} onClick={() => setSelected(b.id)}>
                <div className="bot-item-top">
                  <b>{b.name}</b>
                  <span className={`mode-badge ${b.mode}`}>{b.mode === 'paper' ? '모의' : '실거래'}</span>
                </div>
                <div className="bot-item-sub small">
                  <span className={`dot ${b.enabled ? 'on' : ''}`} /> {b.enabled ? '켜짐' : '꺼짐'} · {b.state.position.qty ? `${b.state.position.qty}주 보유` : '미보유'}
                  <span className={`spacer-r ${cls(p)}`}>{pct(p)}</span>
                </div>
              </button>
            )
          })}
          {!bots.length && (
            <p className="muted small">
              아직 봇이 없습니다. <b>+ 봇 만들기</b>로 시작하거나, 퀀트 랩 백테스트 결과에서 <b>이 설정으로 봇 만들기</b>를 누르세요.
            </p>
          )}
        </aside>
        {current ? (
          <BotDetail bot={current} strategies={strategies} onChanged={upsert}
                     onDeleted={() => { setBots((l) => l.filter((x) => x.id !== current.id)); setSelected(null) }} />
        ) : <div className="placeholder muted">봇을 선택하세요.</div>}
      </div>

      {creating && strategies.length > 0 && (
        <BotForm strategies={strategies} initial={creating} onClose={() => setCreating(null)}
                 onSubmit={async (b) => { const r = await autotradeApi.create(b); upsert(r.bot); setSelected(r.bot.id) }} />
      )}
    </div>
  )
}
