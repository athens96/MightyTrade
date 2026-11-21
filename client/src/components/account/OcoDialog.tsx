import { useState, type FormEvent } from 'react'
import { accountApi, type Holding, type OcoResponse } from '../../api/account'
import { ApiError } from '../../api/client'
import type { Market } from '../../api/types'
import { priceFormatter } from '../chart/format'

const inDays = (d: number) => new Date(Date.now() + d * 86400_000).toISOString().slice(0, 10)

/** Take-profit / stop-loss bracket (Toss OCO) on a holding. Preview (dry run) first; sending needs confirm + OTP. */
export default function OcoDialog({ holding, onClose, onDone }: { holding: Holding; onClose: () => void; onDone: () => void }) {
  const market: Market = holding.currency === 'KRW' ? 'KR' : 'US'
  const fmt = priceFormatter(market)
  const cur = holding.lastPrice
  // snap suggestions to the exchange price grid (server re-checks); KRX tick table since 2023
  const krTick = (v: number) => [[2000, 1], [5000, 5], [20000, 10], [50000, 50], [200000, 100], [500000, 500]].find(([b]) => v < b)?.[1] ?? 1000
  const r = (v: number, mode: 'round' | 'floor' = 'round') =>
    market === 'KR' ? Math[mode](v / krTick(v)) * krTick(v) : Math[mode](v * 100) / 100
  const [qty, setQty] = useState(holding.quantity)
  const [tpTrigger, setTpTrigger] = useState(r(cur * 1.1))
  const [tpPrice, setTpPrice] = useState(r(cur * 1.1))
  const [slTrigger, setSlTrigger] = useState(r(cur * 0.95))
  const [slPrice, setSlPrice] = useState(r(cur * 0.95 * 0.99, 'floor'))
  const [expire, setExpire] = useState(inDays(30))
  const [highValue, setHighValue] = useState(false)
  const [needHighValue, setNeedHighValue] = useState(false)
  const [preview, setPreview] = useState<OcoResponse | null>(null)
  const [confirmed, setConfirmed] = useState(false)
  const [otp, setOtp] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const req = () => ({
    market, symbol: holding.symbol, quantity: qty, expire_date: expire, confirm_high_value: highValue,
    take_profit: { trigger: tpTrigger, price: tpPrice }, stop_loss: { trigger: slTrigger, price: slPrice },
  })
  const pct = (v: number) => `${v >= cur ? '+' : ''}${((v / cur - 1) * 100).toFixed(1)}%`
  const edit = <T,>(set: (v: T) => void) => (v: T) => {
    set(v)
    setPreview(null) // any change invalidates the preview
    setConfirmed(false)
  }

  const run = async (e: FormEvent, live: boolean) => {
    e.preventDefault()
    setBusy(true)
    setError('')
    try {
      const res = await accountApi.oco({ ...req(), dry_run: !live, ...(live ? { otp_code: otp } : {}) })
      if (live) {
        onDone()
        onClose()
      } else {
        setPreview(res)
      }
    } catch (err) {
      if (err instanceof ApiError && err.code === 'confirm_high_value_required') setNeedHighValue(true)
      setError((err as Error).message)
    } finally {
      setOtp('')
      setBusy(false)
    }
  }

  const num = (label: string, value: number, set: (v: number) => void, hint?: string, tip?: string) => (
    <label title={tip}>
      <span>{label} {hint && <span className="muted small">{hint}</span>}</span>
      <input type="number" step="any" value={value} title={tip} onChange={(e) => edit(set)(Number(e.target.value))} />
    </label>
  )

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <form className="modal" onSubmit={(e) => run(e, false)}>
        <h3>익절·손절 걸기 (OCO) · {holding.name}</h3>
        <p className="muted small">
          현재가 {fmt(cur)} · 보유 {holding.quantity.toLocaleString()}주 · 평균단가 {fmt(holding.averagePurchasePrice)}. 가격이 익절 감시가 이상이 되거나
          손절 감시가 이하가 되면 토스증권이 해당 지정가로 매도하고, 나머지 조건은 자동 취소됩니다.
          {market === 'KR' && ' 국내 주식은 KRX 정규장에서만 발동됩니다.'}
        </p>
        <div className="modal-grid">
          {num('수량', qty, setQty, undefined, '익절·손절 자동 감시를 걸어둘 주식 수량입니다 (보유 수량 이내)')}
          <label title="이 날짜까지 익절가나 손절가에 도달하지 않으면 조건주문이 자동 종료됩니다 (최대 1년)">
            <span>만료일 <span className="muted small">최대 1년</span></span>
            <input
              type="date"
              value={expire}
              min={inDays(0)}
              max={inDays(365)}
              title="이 날짜까지 익절가나 손절가에 도달하지 않으면 조건주문이 자동 종료됩니다 (최대 1년)"
              onChange={(e) => edit(setExpire)(e.target.value)}
            />
          </label>
          {num('익절 감시가', tpTrigger, setTpTrigger, pct(tpTrigger), '주가가 이 가격 이상으로 올라오면 수익 실현 매도 주문을 발동시킵니다')}
          {num('익절 주문가 (지정가)', tpPrice, setTpPrice, pct(tpPrice), '익절 감시가에 닿았을 때 실제로 거래소에 내보낼 매도 지정가입니다')}
          {num('손절 감시가', slTrigger, setSlTrigger, pct(slTrigger), '주가가 이 가격 이하로 내려가면 큰 손실을 막기 위해 손절 매도 주문을 발동시킵니다')}
          {num('손절 주문가 (지정가)', slPrice, setSlPrice, `${pct(slPrice)} · 감시가보다 약간 낮게 두면 급락 시에도 체결될 가능성이 높아요`, '손절 감시가에 닿았을 때 내보낼 매도 지정가입니다 (감시가보다 약 1% 낮게 두면 급락 시 미체결 위험을 줄여줍니다)')}
        </div>
        {needHighValue && (
          <label className="check" title="1억 원 이상 큰 금액의 조건주문 실수를 막기 위한 확인 체크입니다">
            <input type="checkbox" checked={highValue} onChange={(e) => edit(setHighValue)(e.target.checked)} /> 1억 원 이상 고액 주문임을 확인합니다
          </label>
        )}

        {!preview && (
          <div className="modal-actions">
            <button type="button" title="입력한 내용을 취소하고 창을 닫습니다" onClick={onClose}>닫기</button>
            <button type="submit" disabled={busy} title="아직 실제 주문을 보내지 않고, 호가 단위 보정 결과와 예상 익절·손절 비율을 먼저 점검합니다">
              {busy ? '확인 중…' : '미리보기 (주문 전송 안 함)'}
            </button>
          </div>
        )}

        {preview && (
          <div className="oco-preview">
            <h4>토스증권에 보낼 내용</h4>
            <table className="metrics">
              <tbody>
                <tr><td>수량</td><td>{preview.payload.quantity}주 (매도 가능 {preview.checks.sellable_quantity}주)</td></tr>
                <tr><td>익절</td><td>{fmt(Number(preview.payload.first.triggerPrice))} 이상 → {fmt(Number(preview.payload.first.orderPrice))}에 매도 ({preview.checks.take_profit_pct >= 0 ? '+' : ''}{preview.checks.take_profit_pct.toFixed(1)}%)</td></tr>
                <tr><td>손절</td><td>{fmt(Number(preview.payload.second.triggerPrice))} 이하 → {fmt(Number(preview.payload.second.orderPrice))}에 매도 ({preview.checks.stop_loss_pct.toFixed(1)}%)</td></tr>
                <tr><td>만료일</td><td>{preview.payload.expireDate}</td></tr>
                <tr><td>현재 평가액</td><td>{fmt(preview.checks.order_value)}</td></tr>
              </tbody>
            </table>
            <p className="muted small">가격은 호가 단위에 맞게 조정되었습니다. 아직 아무 주문도 전송되지 않았습니다.</p>
            <label className="check" title="위 미리보기 조건을 확인했으며 토스증권에 실제 조건주문을 등록한다는 최종 확인 체크입니다">
              <input type="checkbox" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} />
              실제 주문이 토스증권에 전송되는 것을 확인합니다
            </label>
            <div className="modal-actions">
              <button type="button" title="전송하지 않고 창을 닫습니다" onClick={onClose}>닫기</button>
              <input className="otp-input" inputMode="numeric" maxLength={6} placeholder="OTP 6자리" value={otp}
                     title="스마트폰 인증 앱(Google Authenticator 등)에 표시된 6자리 OTP 숫자를 입력하세요"
                     onChange={(e) => setOtp(e.target.value.replace(/\D/g, ''))} />
              <button
                type="button"
                className="danger"
                disabled={busy || !confirmed || otp.length !== 6}
                title="위 확인 체크와 OTP 6자리 입력을 마치면 토스증권에 익절·손절 조건주문을 실제 전송합니다"
                onClick={(e) => run(e, true)}
              >
                {busy ? '전송 중…' : '실제 주문 전송'}
              </button>
            </div>
          </div>
        )}
        {error && <p className="error">{error}</p>}
      </form>
    </div>
  )
}
