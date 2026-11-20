import { NavLink, Outlet } from 'react-router-dom'
import { useAuth } from '../auth/AuthContext'

export default function Layout() {
  const { user, logout } = useAuth()
  return (
    <div className="app">
      <header className="topbar">
        <strong>MightyTrade</strong>
        <nav>
          <NavLink to="/" end>시세/차트</NavLink>
          <NavLink to="/quant">퀀트 랩</NavLink>
          <NavLink to="/account">계좌</NavLink>
          <NavLink to="/auto">자동주문</NavLink>
          <NavLink to="/settings">설정</NavLink>
        </nav>
        <div className="spacer" />
        <span className="muted">{user?.email}</span>
        <button onClick={logout}>로그아웃</button>
      </header>
      <main>
        <Outlet />
      </main>
    </div>
  )
}
