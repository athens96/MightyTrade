import { useEffect, useState } from 'react'
import { tradingApi, type TradingStatus } from '../../api/trading'

/** Trading lock state + OTP unlock (manual orders need it; it lasts a limited time). */
export default function UnlockBar({ onChange }: { onChange?: (s: TradingStatus) => void }) {
  const [status, setStatus] = useState<TradingStatus | null>(null)
  const [otp, setOtp] = useState('')
  const [error, setError] = useState('')

  const apply = (s: TradingStatus) => {
    setStatus(s)
    onChange?.(s)
  }
  useEffect(() => {
    tradingApi.status().then(apply).catch(() => undefined)
    const t = window.setInterval(() => tradingApi.status().then(apply).catch(() => undefined), 60_000)
    return () => window.clearInterval(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const unlock = async () => {
    setError('')
    try {
      apply(await tradingApi.unlock(otp))
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setOtp('')
    }
  }

  if (!status) return null
  const until = status.unlocked_until ? new Date(status.unlocked_until).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit', hour12: false }) : ''
  return (
    <div className={`unlock-bar ${status.unlocked ? 'on' : ''}`}>
      {status.unlocked ? (
        <>
          <span>🔓 거래 가능 ~{until}</span>
          <button className="link" onClick={async () => apply(await tradingApi.lock())}>잠그기</button>
        </>
      ) : (
        <>
          <span>🔒 거래 잠김</span>
          <input className="otp-input" inputMode="numeric" maxLength={6} placeholder="OTP" value={otp}
                 onChange={(e) => setOtp(e.target.value.replace(/\D/g, ''))} onKeyDown={(e) => e.key === 'Enter' && otp.length === 6 && unlock()} />
          <button disabled={otp.length !== 6} onClick={unlock}>{status.limits.unlock_minutes}분 해제</button>
        </>
      )}
      <span className="muted small">오늘 {status.today_orders}/{status.limits.max_orders_per_day}건</span>
      {error && <span className="error small">{error}</span>}
    </div>
  )
}
