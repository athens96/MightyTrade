import { lazy, Suspense } from 'react'
import { Navigate, Route, Routes } from 'react-router-dom'
import HoverTooltip from './components/HoverTooltip'
import Layout from './components/Layout'
import ProtectedRoute from './components/ProtectedRoute'
import VersionUpdateModal from './components/VersionUpdateModal'
import LoginPage from './pages/LoginPage'
import OtpPage from './pages/OtpPage'
import DashboardPage from './pages/DashboardPage'
import AccountPage from './pages/AccountPage'
import AdminPage from './pages/AdminPage'
import AutoTradePage from './pages/AutoTradePage'
import SettingsPage from './pages/SettingsPage'

// Loaded on demand to keep the main bundle small.
const QuantPage = lazy(() => import('./pages/QuantPage'))

export default function App() {
  return (
    <>
      <HoverTooltip />
      <VersionUpdateModal />
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/otp" element={<OtpPage />} />
        <Route element={<ProtectedRoute />}>
          <Route element={<Layout />}>
            <Route path="/" element={<DashboardPage />} />
            <Route path="/quant" element={<Suspense fallback={<div className="placeholder muted">불러오는 중…</div>}><QuantPage /></Suspense>} />
            <Route path="/account" element={<AccountPage />} />
            <Route path="/admin" element={<AdminPage />} />
            <Route path="/orders" element={<Navigate to="/account" replace />} />
            <Route path="/auto" element={<AutoTradePage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </>
  )
}
