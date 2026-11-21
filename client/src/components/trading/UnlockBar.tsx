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
          <button className="link" title="실수로 주문이 나가지 않도록 즉시 거래 잠금 상태로 되돌립니다" onClick={async () => apply(await tradingApi.lock())}>잠그기</button>
        </>
      ) : (
        <>
          <span>🔒 거래 잠김</span>
          <input className="otp-input" inputMode="numeric" maxLength={6} placeholder="OTP" value={otp}
                 title="스마트폰 인증 앱(Google Authenticator 등)에 표시된 6자리 일회용 숫자를 입력하세요"
                 onChange={(e) => setOtp(e.target.value.replace(/\D/g, ''))} onKeyDown={(e) => e.key === 'Enter' && otp.length === 6 && unlock()} />
          <button
            disabled={otp.length !== 6}
            title={`OTP 6자리를 확인해 ${status.limits.unlock_minutes}분 동안 실제 매수·매도·정정 주문을 보낼 수 있도록 잠금을 풉니다`}
            onClick={unlock}
          >
            {status.limits.unlock_minutes}분 해제
          </button>
        </>
      )}
      <span className="muted small">오늘 {status.today_orders}/{status.limits.max_orders_per_day}건</span>
      {error && <span className="error small">{error}</span>}
    </div>
  )
}
