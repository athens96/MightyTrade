import { useEffect, useState } from 'react'
import { NavLink, Outlet, useLocation } from 'react-router-dom'
import { adminApi } from '../api/admin'
import { useAuth } from '../auth/AuthContext'
import { useOrderEvents } from '../hooks/useOrderEvents'
import { APP_VERSION } from '../lib/version'
import RequestModal from './RequestModal'
import { OPEN_UPDATE_MODAL_EVENT, UPDATE_AVAILABLE_EVENT } from './VersionUpdateModal'

const MAIN_TABS = [
  { to: '/', label: '시세', long: '시세/차트', icon: '📈', end: true, tip: '국내·미국 실시간 차트와 호가를 보고 직접 매수·매도 주문을 넣는 화면입니다.' },
  { to: '/quant', label: '퀀트', long: '퀀트 랩', icon: '🧪', tip: '유망 종목 발굴부터 백테스트·최적화·포트폴리오 실험까지 내 전략을 검증하는 실험실입니다.' },
  { to: '/account', label: '계좌', long: '계좌', icon: '💼', tip: '보유 종목 수익률, 매수 가능 현금, 미체결 주문, 익절·손절(OCO) 예약을 관리합니다.' },
  { to: '/auto', label: '자동매매', long: '자동매매', icon: '🤖', tip: '스윙봇·단타봇·자산배분 리밸런싱 봇을 모의 또는 실거래로 자동 운용합니다.' },
]

export default function Layout() {
  const { user, logout } = useAuth()
  const toasts = useOrderEvents()
  const isAdmin = user?.role === 'admin'
  const [adminCounts, setAdminCounts] = useState({ pending: 0, openRequests: 0 })
  const [more, setMore] = useState(false)
  const [requestOpen, setRequestOpen] = useState(false)
  const [newerVer, setNewerVer] = useState<string | null>(null)
  const location = useLocation()

  useEffect(() => setMore(false), [location.pathname])
  useEffect(() => {
    const onUpdate = (e: Event) => {
      const v = (e as CustomEvent<{ version?: string }>).detail?.version
      if (v) setNewerVer(v)
    }
    window.addEventListener(UPDATE_AVAILABLE_EVENT, onUpdate)
    return () => window.removeEventListener(UPDATE_AVAILABLE_EVENT, onUpdate)
  }, [])
  useEffect(() => {
    if (!isAdmin) return
    const load = () =>
      adminApi
        .summary()
        .then((r) => setAdminCounts({ pending: r.pending, openRequests: r.open_requests ?? 0 }))
        .catch(() => undefined)
    load()
    const t = window.setInterval(load, 60_000)
    window.addEventListener('mt:admin-changed', load)
    return () => {
      window.clearInterval(t)
      window.removeEventListener('mt:admin-changed', load)
    }
  }, [isAdmin])

  const adminTotal = adminCounts.pending + adminCounts.openRequests
  const badgeTip = `승인 대기 ${adminCounts.pending}명 · 미처리 요청사항 ${adminCounts.openRequests}건`
  const badge = adminTotal > 0 && <span className="nav-badge" title={badgeTip}>{adminTotal}</span>
  const moreActive = ['/settings', '/admin'].some((p) => location.pathname.startsWith(p))

  return (
    <div className="app">
      {/* three columns: menu | centered title | user — the title stays centred whatever the menu width */}
      <header className="topbar">
        <nav className="topbar-left desktop-only">
          {MAIN_TABS.map((t) => <NavLink key={t.to} to={t.to} end={t.end} title={t.tip}>{t.long}</NavLink>)}
          <NavLink to="/settings" title="내 토스증권 Open API 키와 실시간 알림을 안전하게 등록·확인하는 설정 화면입니다.">설정</NavLink>
          {isAdmin && <NavLink to="/admin" title="가입 신청 승인 및 사용자 요청사항을 처리하는 관리자 전용 화면입니다.">관리자{badge}</NavLink>}
        </nav>
        <strong className="brand">
          <img src="/favicon.svg" alt="" width={24} height={24} />
          <span>Mighty Trade</span>
          {newerVer ? (
            <button
              type="button"
              className="ver-badge update"
              title={`새 버전(v${newerVer})이 준비되었습니다. 클릭하면 업데이트 안내창을 엽니다`}
              onClick={() => window.dispatchEvent(new Event(OPEN_UPDATE_MODAL_EVENT))}
            >
              🔄 v{newerVer} 업데이트
            </button>
          ) : (
            <span className="ver-badge" title={`현재 실행 중인 마이티 트레이드 버전 (v${APP_VERSION})`}>v{APP_VERSION}</span>
          )}
        </strong>
        <div className="topbar-right">
          <button
            type="button"
            className="desktop-only"
            onClick={() => setRequestOpen(true)}
            title="기능 건의나 오류 제보를 관리자에게 직접 보냅니다 (관리자만 열람 가능)"
          >
            💬 요청사항
          </button>
          <span className="muted desktop-only" title="현재 로그인된 Google 계정 이메일입니다.">{user?.email}</span>
          <button className="desktop-only" onClick={logout} title="현재 기기에서 안전하게 로그아웃합니다.">로그아웃</button>
        </div>
      </header>
      <main>
        <Outlet />
      </main>

      {/* mobile navigation: bottom tab bar + "more" sheet */}
      <nav className="tabbar mobile-only" aria-label="주요 메뉴">
        {MAIN_TABS.map((t) => (
          <NavLink key={t.to} to={t.to} end={t.end} title={t.tip}>
            <span className="tab-icon" aria-hidden>{t.icon}</span>
            <span>{t.label}</span>
          </NavLink>
        ))}
        <button className={moreActive || more ? 'active' : ''} onClick={() => setMore((v) => !v)} aria-expanded={more} title="요청사항·설정·관리자·로그아웃 메뉴를 엽니다.">
          <span className="tab-icon" aria-hidden>☰</span>
          <span>더보기{badge}</span>
        </button>
      </nav>
      {more && (
        <div className="sheet-backdrop mobile-only" onClick={() => setMore(false)}>
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <div className="sheet-user">
              <b>{user?.name ?? '사용자'} <span className="ver-badge">v{APP_VERSION}</span></b>
              <span className="muted small">{user?.email}</span>
            </div>
            <button
              type="button"
              title="기능 건의나 오류 제보를 관리자에게 직접 보냅니다 (관리자만 열람 가능)"
              onClick={() => { setMore(false); setRequestOpen(true) }}
            >
              💬 요청사항 보내기
            </button>
            <NavLink to="/settings">⚙️ 설정 · 토스 API 연결</NavLink>
            {isAdmin && <NavLink to="/admin">🛡️ 관리자 · 계정 및 요청사항 {badge}</NavLink>}
            <button onClick={logout}>로그아웃</button>
          </div>
        </div>
      )}

      {requestOpen && <RequestModal currentPage={location.pathname} onClose={() => setRequestOpen(false)} />}

      <div className="toasts" aria-live="polite">
        {toasts.map((t) => <div key={t.id} className={`toast ${t.tone}`}>{t.text}</div>)}
      </div>
    </div>
  )
}
