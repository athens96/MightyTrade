import { useEffect, useState } from 'react'
import { NavLink, Outlet, useLocation } from 'react-router-dom'
import { adminApi } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { useOrderEvents } from '../hooks/useOrderEvents'

const MAIN_TABS = [
  { to: '/', label: '시세', long: '시세/차트', icon: '📈', end: true },
  { to: '/quant', label: '퀀트', long: '퀀트 랩', icon: '🧪' },
  { to: '/account', label: '계좌', long: '계좌', icon: '💼' },
  { to: '/auto', label: '자동매매', long: '자동매매', icon: '🤖' },
]

export default function Layout() {
  const { user, logout } = useAuth()
  const toasts = useOrderEvents()
  const isAdmin = user?.role === 'admin'
  const [pending, setPending] = useState(0)
  const [more, setMore] = useState(false)
  const location = useLocation()

  useEffect(() => setMore(false), [location.pathname])
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

  const badge = pending > 0 && <span className="nav-badge" title="승인 대기">{pending}</span>
  const moreActive = ['/settings', '/admin'].some((p) => location.pathname.startsWith(p))

  return (
    <div className="app">
      {/* three columns: menu | centered title | user — the title stays centred whatever the menu width */}
      <header className="topbar">
        <nav className="topbar-left desktop-only">
          {MAIN_TABS.map((t) => <NavLink key={t.to} to={t.to} end={t.end}>{t.long}</NavLink>)}
          <NavLink to="/settings">설정</NavLink>
          {isAdmin && <NavLink to="/admin">관리자{badge}</NavLink>}
        </nav>
        <strong className="brand">
          <img src="/favicon.svg" alt="" width={24} height={24} />
          <span>Mighty Trade</span>
        </strong>
        <div className="topbar-right">
          <span className="muted desktop-only">{user?.email}</span>
          <button className="desktop-only" onClick={logout}>로그아웃</button>
        </div>
      </header>
      <main>
        <Outlet />
      </main>

      {/* mobile navigation: bottom tab bar + "more" sheet */}
      <nav className="tabbar mobile-only" aria-label="주요 메뉴">
        {MAIN_TABS.map((t) => (
          <NavLink key={t.to} to={t.to} end={t.end}>
            <span className="tab-icon" aria-hidden>{t.icon}</span>
            <span>{t.label}</span>
          </NavLink>
        ))}
        <button className={moreActive || more ? 'active' : ''} onClick={() => setMore((v) => !v)} aria-expanded={more}>
          <span className="tab-icon" aria-hidden>☰</span>
          <span>더보기{badge}</span>
        </button>
      </nav>
      {more && (
        <div className="sheet-backdrop mobile-only" onClick={() => setMore(false)}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <div className="sheet-user">
              <b>{user?.name ?? '사용자'}</b>
              <span className="muted small">{user?.email}</span>
            </div>
            <NavLink to="/settings">⚙️ 설정 · 토스 API 연결</NavLink>
            {isAdmin && <NavLink to="/admin">🛡️ 관리자 · 사용자 관리 {badge}</NavLink>}
            <button onClick={logout}>로그아웃</button>
          </div>
        </div>
      )}

      <div className="toasts" aria-live="polite">
        {toasts.map((t) => <div key={t.id} className={`toast ${t.tone}`}>{t.text}</div>)}
      </div>
    </div>
  )
}
