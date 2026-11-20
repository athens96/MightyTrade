import { NavLink, Outlet } from 'react-router-dom'
import { useAuth } from '../auth/AuthContext'
import { useOrderEvents } from '../hooks/useOrderEvents'

export default function Layout() {
  const { user, logout } = useAuth()
  const toasts = useOrderEvents()
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
