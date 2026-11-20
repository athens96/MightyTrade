import { useCallback, useEffect, useState } from 'react'
import { Navigate } from 'react-router-dom'
import { adminApi, type AdminUser, type UserFilter } from '../api/admin'
import { ApiError } from '../api/client'
import { useAuth } from '../auth/AuthContext'

const TABS: { id: UserFilter; label: string }[] = [
  { id: 'pending', label: '승인 대기' },
  { id: 'approved', label: '승인됨' },
  { id: 'rejected', label: '거절됨' },
  { id: 'all', label: '전체' },
]
const STATUS: Record<string, string> = { pending: '승인 대기', approved: '승인됨', rejected: '거절됨' }
const when = (iso: string | null) =>
  iso ? new Date(iso).toLocaleString('ko-KR', { year: '2-digit', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hour12: false }) : '-'

function DeleteDialog({ user, onClose, onDone }: { user: AdminUser; onClose: () => void; onDone: () => void }) {
  const [otp, setOtp] = useState('')
  const [typed, setTyped] = useState('')
  const [needForce, setNeedForce] = useState(false)
  const [force, setForce] = useState(false)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const submit = async () => {
    setBusy(true)
    setError('')
    try {
      await adminApi.remove(user.id, otp, force)
      onDone()
      onClose()
    } catch (e) {
      if (e instanceof ApiError && e.code === 'live_positions') setNeedForce(true)
      setError((e as Error).message)
    } finally {
      setOtp('')
      setBusy(false)
    }
  }
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal">
        <h3>계정 삭제 · {user.email}</h3>
        <p className="small">
          이 계정과 데이터가 <b>영구 삭제</b>됩니다: 프로필·OTP, 관심종목·설정, 토스 API 키, 자동매매 봇({user.bots}개)과 기록, 주문 기록.
          토스증권 계좌의 실제 주문·보유는 영향을 받지 않습니다. 같은 Google 계정으로 다시 로그인하면 새 가입 신청이 됩니다.
        </p>
        <label className="modal-grid" style={{ gridTemplateColumns: '1fr' }}>
          <span className="small muted">확인을 위해 이메일을 입력하세요</span>
          <input className="otp-input" style={{ width: '100%' }} value={typed} onChange={(e) => setTyped(e.target.value)} placeholder={user.email} />
        </label>
        {needForce && (
          <label className="check small">
            <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} /> 실거래 봇 보유·미체결 주문 추적을 포기하고 삭제합니다
          </label>
        )}
        <div className="modal-actions">
          <button onClick={onClose}>취소</button>
          <input className="otp-input" inputMode="numeric" maxLength={6} placeholder="OTP" value={otp} onChange={(e) => setOtp(e.target.value.replace(/\D/g, ''))} />
          <button className="danger" disabled={busy || otp.length !== 6 || typed.trim().toLowerCase() !== (user.email ?? '').toLowerCase() || (needForce && !force)} onClick={submit}>
            {busy ? '삭제 중…' : '영구 삭제'}
          </button>
        </div>
        {error && <p className="error small">{error}</p>}
      </div>
    </div>
  )
}

export default function AdminPage() {
  const { user } = useAuth()
  const [tab, setTab] = useState<UserFilter>('pending')
  const [items, setItems] = useState<AdminUser[]>([])
  const [counts, setCounts] = useState<Record<UserFilter, number> | null>(null)
  const [deleting, setDeleting] = useState<AdminUser | null>(null)
  const [error, setError] = useState('')

  const load = useCallback(() => {
    adminApi.users(tab).then((r) => {
      setItems(r.items)
      setCounts(r.counts)
      setError('')
      window.dispatchEvent(new Event('mt:admin-changed'))
    }).catch((e) => setError(e.message))
  }, [tab])
  useEffect(() => load(), [load])

  if (user && user.role !== 'admin') return <Navigate to="/" replace />

  const act = async (fn: () => Promise<unknown>) => {
    try {
      await fn()
      load()
    } catch (e) {
      alert((e as Error).message)
    }
  }

  return (
    <div className="account">
      <div className="account-head">
        <h2>관리자 · 사용자 관리</h2>
        <div className="tabs">
          {TABS.map((t) => (
            <button key={t.id} className={tab === t.id ? 'active' : ''} onClick={() => setTab(t.id)}>
              {t.label}{counts ? ` ${counts[t.id]}` : ''}
            </button>
          ))}
        </div>
        <div className="spacer" />
        <button onClick={load}>새로고침</button>
      </div>
      <p className="muted small">
        새 Google 로그인은 승인 대기로 등록됩니다. 승인 전에는 OTP 등록과 모든 기능 이용이 막혀 있습니다. 관리자 계정은 서버 설정(ADMIN_EMAILS)으로 정해집니다.
      </p>
      {error && <p className="error">{error}</p>}
      <div className="trades-wrap">
        <table className="trades left-first admin-users cards">
          <thead>
            <tr><th>사용자</th><th>상태</th><th>가입 신청</th><th>마지막 로그인</th><th>OTP</th><th>토스 키</th><th>봇</th><th /></tr>
          </thead>
          <tbody>
            {items.map((u) => {
              const self = u.id === user?.id
              const admin = u.role === 'admin'
              return (
                <tr key={u.id}>
                  <td className="card-title">
                    <div className="user-cell">
                      {u.picture ? <img src={u.picture} alt="" referrerPolicy="no-referrer" /> : <span className="avatar">{(u.name ?? u.email ?? '?')[0]}</span>}
                      <div>
                        <b>{u.name ?? '-'}</b> {admin && <span className="mode-badge">관리자</span>}{self && <span className="muted small"> (나)</span>}
                        <div className="muted small">{u.email}</div>
                      </div>
                    </div>
                  </td>
                  <td data-label="상태"><span className={`status-badge ${u.status}`}>{STATUS[u.status] ?? u.status}</span></td>
                  <td data-label="가입 신청" className="muted">{when(u.created_at)}</td>
                  <td data-label="마지막 로그인" className="muted">{when(u.last_login_at)}</td>
                  <td data-label="OTP">{u.otp_enabled ? '등록' : '-'}</td>
                  <td data-label="토스 키">{u.has_toss_key ? '등록' : '-'}</td>
                  <td data-label="봇">{u.bots ? `${u.bots}개${u.live_bots ? ` · 실거래 ${u.live_bots}` : ''}` : '-'}</td>
                  <td className="card-actions actions">
                    {u.status !== 'approved' && <button className="primary" onClick={() => act(() => adminApi.approve(u.id))}>승인</button>}
                    {!self && !admin && u.status !== 'rejected' && (
                      <button onClick={() => confirm(`${u.email} ${u.status === 'approved' ? '승인을 취소' : '가입을 거절'}할까요?`) && act(() => adminApi.reject(u.id))}>
                        {u.status === 'approved' ? '승인 취소' : '거절'}
                      </button>
                    )}
                    {!self && !admin && <button className="link danger-link" onClick={() => setDeleting(u)}>삭제</button>}
                  </td>
                </tr>
              )
            })}
            {!items.length && <tr><td colSpan={8} className="muted">{tab === 'pending' ? '승인 대기 중인 가입 신청이 없습니다.' : '해당하는 사용자가 없습니다.'}</td></tr>}
          </tbody>
        </table>
      </div>
      {deleting && <DeleteDialog user={deleting} onClose={() => setDeleting(null)} onDone={load} />}
    </div>
  )
}
