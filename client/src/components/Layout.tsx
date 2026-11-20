import { useEffect, useState } from 'react'
import { NavLink, Outlet } from 'react-router-dom'
import { adminApi } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { useOrderEvents } from '../hooks/useOrderEvents'

export default function Layout() {
  const { user, logout } = useAuth()
  const toasts = useOrderEvents()
  const isAdmin = user?.role === 'admin'
  const [pending, setPending] = useState(0)
  useEffect(() => {
    if (!isAdmin) return
    const load = () => adminApi.summary().then((r) => setPending(r.pending)).catch(() => undefined)
    load()
    const t = window.setInterval(load, 60_000)
    window.addEventListener('mt:admin-changed', load)
    return () => {
      window.clearInterval(t)
      window.removeEventListener('mt:admin-changed', load)
    }
  }, [isAdmin])
  return (
    <div className="app">
      <header className="topbar">
        <strong>MightyTrade</strong>
        <nav>
          <NavLink to="/" end>시세/차트</NavLink>
          <NavLink to="/quant">퀀트 랩</NavLink>
          <NavLink to="/account">계좌</NavLink>
          <NavLink to="/auto">자동매매</NavLink>
          <NavLink to="/settings">설정</NavLink>
          {isAdmin && (
            <NavLink to="/admin">
              관리자{pending > 0 && <span className="nav-badge" title="승인 대기">{pending}</span>}
            </NavLink>
          )}
        </nav>
        <div className="spacer" />
        <span className="muted">{user?.email}</span>
        <button onClick={logout}>로그아웃</button>
      </header>
      <main>
        <Outlet />
      </main>
      <div className="toasts" aria-live="polite">
        {toasts.map((t) => <div key={t.id} className={`toast ${t.tone}`}>{t.text}</div>)}
      </div>
    </div>
  )
}
