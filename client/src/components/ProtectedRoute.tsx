import { Navigate, Outlet } from 'react-router-dom'
import { useAuth } from '../auth/AuthContext'

export default function ProtectedRoute() {
  const { status } = useAuth()
  if (status === 'loading') return <div className="center">Loading…</div>
  if (status === 'pending') return <Navigate to="/otp" replace />
  if (status !== 'authenticated') return <Navigate to="/login" replace />
  return <Outlet />
}
