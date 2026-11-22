import { Navigate, Outlet, useLocation } from 'react-router-dom'
import { saveReturnTo, useAuth } from '../auth/AuthContext'

export default function ProtectedRoute() {
  const { status } = useAuth()
  const location = useLocation()
  if (status === 'loading') return <div className="center">Loading…</div>
  if (status === 'pending') return <Navigate to="/otp" replace />
  if (status !== 'authenticated') {
    saveReturnTo(`${location.pathname}${location.search}`)
    return <Navigate to="/login" replace />
  }
  return <Outlet />
}
