import { useCallback, useEffect, useState } from 'react'
import { Navigate } from 'react-router-dom'
import { adminApi, type AdminRequestItem, type AdminUser, type RequestFilter, type UserFilter } from '../api/admin'
import { ApiError } from '../api/client'
import { REQUEST_CATEGORY_LABEL, REQUEST_STATUS_LABEL, type RequestStatus } from '../api/requests'
import { useAuth } from '../auth/AuthContext'

const USER_TABS: { id: UserFilter; label: string }[] = [
  { id: 'pending', label: '승인 대기' },
  { id: 'approved', label: '승인됨' },
  { id: 'rejected', label: '거절됨' },
  { id: 'all', label: '전체' },
]

const REQ_TABS: { id: RequestFilter; label: string }[] = [
  { id: 'open', label: '접수 대기' },
  { id: 'in_review', label: '검토 중' },
  { id: 'resolved', label: '반영 완료' },
  { id: 'closed', label: '보류·종료' },
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
          이 계정과 데이터가 <b>영구 삭제</b>됩니다: 프로필·OTP, 관심종목·설정, 토스 API 키, 자동매매 봇({user.bots}개)과 기록, 주문 기록, 요청사항.
          토스증권 계좌의 실제 주문·보유는 영향을 받지 않습니다. 같은 Google 계정으로 다시 로그인하면 새 가입 신청이 됩니다.
        </p>
        <label className="modal-grid" style={{ gridTemplateColumns: '1fr' }} title="실수로 다른 계정을 삭제하지 않도록 삭제 대상 이메일 주소를 똑같이 입력하세요">
          <span className="small muted">확인을 위해 이메일을 입력하세요</span>
          <input
            className="otp-input"
            style={{ width: '100%' }}
            value={typed}
            title="삭제할 계정의 이메일 주소를 정확히 입력하세요"
            onChange={(e) => setTyped(e.target.value)}
            placeholder={user.email}
          />
        </label>
        {needForce && (
          <label className="check small" title="실거래 봇 기록까지 강제로 지우고 계정을 영구 삭제하는 데 동의합니다">
            <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} /> 실거래 봇 보유·미체결 주문 추적을 포기하고 삭제합니다
          </label>
        )}
        <div className="modal-actions">
          <button title="삭제하지 않고 창을 닫습니다" onClick={onClose}>취소</button>
          <input
            className="otp-input"
            inputMode="numeric"
            maxLength={6}
            placeholder="OTP"
            value={otp}
            title="관리자 본인 확인을 위해 6자리 OTP 숫자를 입력하세요"
            onChange={(e) => setOtp(e.target.value.replace(/\D/g, ''))}
          />
          <button
            className="danger"
            disabled={busy || otp.length !== 6 || typed.trim().toLowerCase() !== (user.email ?? '').toLowerCase() || (needForce && !force)}
            title="이메일과 관리자 OTP가 일치하면 이 계정과 앱 내 데이터를 영구 삭제합니다"
            onClick={submit}
          >
            {busy ? '삭제 중…' : '영구 삭제'}
          </button>
        </div>
        {error && <p className="error small">{error}</p>}
      </div>
    </div>
  )
}

function AdminRequestCard({ item, onChanged }: { item: AdminRequestItem; onChanged: () => void }) {
  const [status, setStatus] = useState<RequestStatus>(item.status)
  const [reply, setReply] = useState(item.admin_reply)
  const [busy, setBusy] = useState(false)
  const [savedToast, setSavedToast] = useState(false)

  useEffect(() => {
    setStatus(item.status)
    setReply(item.admin_reply)
  }, [item.id, item.status, item.admin_reply])

  const dirty = status !== item.status || reply.trim() !== (item.admin_reply ?? '').trim()

  const save = async () => {
    setBusy(true)
    setSavedToast(false)
    try {
      await adminApi.updateRequest(item.id, { status, admin_reply: reply })
      setSavedToast(true)
      onChanged()
      window.setTimeout(() => setSavedToast(false), 2500)
    } catch (e) {
      alert((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!confirm(`"${item.title}" 요청사항을 삭제할까요?`)) return
    setBusy(true)
    try {
      await adminApi.deleteRequest(item.id)
      onChanged()
    } catch (e) {
      alert((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="panel req-card admin">
      <div className="req-card-head">
        <span className="cat-badge">{REQUEST_CATEGORY_LABEL[item.category] ?? item.category}</span>
        <span className={`status-badge ${item.status}`}>{REQUEST_STATUS_LABEL[item.status] ?? item.status}</span>
        <b className="req-title">{item.title}</b>
        <span className="muted small spacer-r">{when(item.created_at)}</span>
      </div>

      <div className="req-meta muted small">
        <span>보낸 사람: <b className="plain">{item.user_name ?? '-'}</b> ({item.user_email ?? '알 수 없음'})</span>
        {item.page && <span> · 화면: <code>{item.page}</code></span>}
        {item.app_version && <span> · 버전: <code>v{item.app_version}</code></span>}
        {item.replied_by && item.replied_at && <span> · 최근 처리: {item.replied_by} ({when(item.replied_at)})</span>}
      </div>

      <p className="req-content">{item.content}</p>

      <div className="req-admin-box">
        <textarea
          rows={2}
          value={reply}
          maxLength={2000}
          placeholder="작성자에게 보여줄 답변·반영 안내를 입력하세요 (작성자 본인에게만 표시됩니다)"
          title="작성자의 '내가 보낸 요청' 화면에 표시될 관리자 답변을 입력합니다"
          onChange={(e) => setReply(e.target.value)}
        />
        <div className="req-admin-row">
          <select
            value={status}
            title="이 요청사항의 처리 진행 상태를 선택합니다"
            onChange={(e) => setStatus(e.target.value as RequestStatus)}
          >
            {(Object.keys(REQUEST_STATUS_LABEL) as RequestStatus[]).map((s) => (
              <option key={s} value={s}>{REQUEST_STATUS_LABEL[s]}</option>
            ))}
          </select>
          <button
            type="button"
            className="primary"
            disabled={busy || !dirty}
            title="선택한 처리 상태와 답변 내용을 저장합니다"
            onClick={save}
          >
            {busy ? '저장 중…' : '상태·답변 저장'}
          </button>
          {savedToast && <span className="ok small">✓ 저장됨</span>}
          <div className="spacer" />
          <button
            type="button"
            className="link danger-link"
            disabled={busy}
            title="이 요청사항을 수신함에서 영구 삭제합니다"
            onClick={remove}
          >
            삭제
          </button>
        </div>
      </div>
    </div>
  )
}

export default function AdminPage() {
  const { user } = useAuth()
  const [section, setSection] = useState<'users' | 'requests'>('users')
  const [tab, setTab] = useState<UserFilter>('pending')
  const [items, setItems] = useState<AdminUser[]>([])
  const [counts, setCounts] = useState<Record<UserFilter, number> | null>(null)
  const [deleting, setDeleting] = useState<AdminUser | null>(null)

  const [reqTab, setReqTab] = useState<RequestFilter>('open')
  const [reqItems, setReqItems] = useState<AdminRequestItem[]>([])
  const [reqCounts, setReqCounts] = useState<Record<RequestFilter, number> | null>(null)
  const [error, setError] = useState('')

  const loadUsers = useCallback(() => {
    adminApi.users(tab).then((r) => {
      setItems(r.items)
      setCounts(r.counts)
      setError('')
      window.dispatchEvent(new Event('mt:admin-changed'))
    }).catch((e) => setError(e.message))
  }, [tab])

  const loadRequests = useCallback(() => {
    adminApi.requests(reqTab).then((r) => {
      setReqItems(r.items)
      setReqCounts(r.counts)
      setError('')
      window.dispatchEvent(new Event('mt:admin-changed'))
    }).catch((e) => setError(e.message))
  }, [reqTab])

  useEffect(() => {
    loadUsers()
  }, [loadUsers])

  useEffect(() => {
    loadRequests()
  }, [loadRequests])

  if (user && user.role !== 'admin') return <Navigate to="/" replace />

  const act = async (fn: () => Promise<unknown>) => {
    try {
      await fn()
      loadUsers()
    } catch (e) {
      alert((e as Error).message)
    }
  }

  const pendingBadge = counts && counts.pending > 0 ? <span className="nav-badge">{counts.pending}</span> : null
  const openReqBadge = reqCounts && reqCounts.open > 0 ? <span className="nav-badge">{reqCounts.open}</span> : null

  return (
    <div className="account">
      <div className="account-head">
        <h2>관리자 센터</h2>
        <div className="spacer" />
        <button
          title="현재 보고 있는 관리자 목록을 최신 상태로 새로고침합니다"
          onClick={() => (section === 'users' ? loadUsers() : loadRequests())}
        >
          새로고침
        </button>
      </div>

      <div className="tabs">
        <button
          type="button"
          className={section === 'users' ? 'active' : ''}
          title="신규 Google 로그인 가입 승인 및 사용자 계정을 관리합니다"
          onClick={() => setSection('users')}
        >
          👥 가입 승인 · 계정 관리{pendingBadge}
        </button>
        <button
          type="button"
          className={section === 'requests' ? 'active' : ''}
          title="사용자들이 보낸 기능 건의·오류 제보 수신함을 확인하고 답변합니다 (관리자 전용)"
          onClick={() => setSection('requests')}
        >
          📩 사용자 요청사항 수신함{openReqBadge}
        </button>
      </div>

      {error && <p className="error">{error}</p>}

      {section === 'users' ? (
        <>
          <div className="tabs">
            {USER_TABS.map((t) => (
              <button key={t.id} className={tab === t.id ? 'active' : ''} title={`${t.label} 상태의 사용자 목록을 확인합니다`} onClick={() => setTab(t.id)}>
                {t.label}{counts ? ` ${counts[t.id]}` : ''}
              </button>
            ))}
          </div>
          <p className="muted small">
            새 Google 로그인은 승인 대기로 등록됩니다. 승인 전에는 OTP 등록과 모든 기능 이용이 막혀 있습니다. 관리자 계정은 서버 설정(ADMIN_EMAILS)으로 정해집니다.
          </p>
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
                        {u.status !== 'approved' && (
                          <button className="primary" title="이 사용자의 가입을 승인해 OTP 등록과 서비스 이용을 허용합니다" onClick={() => act(() => adminApi.approve(u.id))}>승인</button>
                        )}
                        {!self && !admin && u.status !== 'rejected' && (
                          <button
                            title={u.status === 'approved' ? '이 사용자의 서비스 이용 승인을 취소하고 접근을 차단합니다' : '이 가입 신청을 거절합니다'}
                            onClick={() => confirm(`${u.email} ${u.status === 'approved' ? '승인을 취소' : '가입을 거절'}할까요?`) && act(() => adminApi.reject(u.id))}
                          >
                            {u.status === 'approved' ? '승인 취소' : '거절'}
                          </button>
                        )}
                        {!self && !admin && <button className="link danger-link" title="이 사용자 계정과 앱 내 저장 데이터를 영구 삭제합니다" onClick={() => setDeleting(u)}>삭제</button>}
                      </td>
                    </tr>
                  )
                })}
                {!items.length && <tr><td colSpan={8} className="muted">{tab === 'pending' ? '승인 대기 중인 가입 신청이 없습니다.' : '해당하는 사용자가 없습니다.'}</td></tr>}
              </tbody>
            </table>
          </div>
          {deleting && <DeleteDialog user={deleting} onClose={() => setDeleting(null)} onDone={loadUsers} />}
        </>
      ) : (
        <>
          <div className="tabs">
            {REQ_TABS.map((t) => (
              <button key={t.id} className={reqTab === t.id ? 'active' : ''} title={`${t.label} 상태의 요청사항을 모아 봅니다`} onClick={() => setReqTab(t.id)}>
                {t.label}{reqCounts ? ` ${reqCounts[t.id]}` : ''}
              </button>
            ))}
          </div>
          <p className="muted small">
            사용자가 상단 <b>💬 요청사항</b>으로 보낸 건의·제보입니다. 이 수신함은 오직 관리자만 열람할 수 있으며, 관리자 계정의 <b>설정 → 실시간 알림(텔레그램·디스코드·슬랙)</b>이 켜져 있으면 새 요청 접수 즉시 알림이 발송됩니다.
          </p>
          <div className="req-list">
            {reqItems.map((r) => (
              <AdminRequestCard key={r.id} item={r} onChanged={loadRequests} />
            ))}
            {!reqItems.length && (
              <div className="panel muted small">
                {reqTab === 'open' ? '미처리(접수 대기) 상태인 사용자 요청사항이 없습니다.' : '해당 상태의 사용자 요청사항이 없습니다.'}
              </div>
            )}
          </div>
        </>
      )}
    </div>
  )
}
