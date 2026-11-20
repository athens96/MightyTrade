import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { accountApi, type AccountSummary, type ConditionalLeg, type ConditionalOrder, type Holding } from '../api/account'
import { ApiError } from '../api/client'
import OcoDialog from '../components/account/OcoDialog'
import { FeatureGuideBox } from '../components/GuideBox'
import OrdersSection from '../components/trading/OrdersSection'
import { priceFormatter } from '../components/chart/format'
import { FEATURE_GUIDES } from '../lib/featureGuides'

const krw = priceFormatter('KR')
const usd = priceFormatter('US')
const money = (v: number | null | undefined, currency: 'KRW' | 'USD') =>
  v === null || v === undefined ? '-' : currency === 'KRW' ? `${krw(v)}원` : `$${usd(v)}`
const signed = (v: number | null | undefined, currency: 'KRW' | 'USD') =>
  v === null || v === undefined ? '-' : `${v > 0 ? '+' : v < 0 ? '-' : ''}${money(Math.abs(v), currency)}`
const pct = (v: number | null | undefined) => (v === null || v === undefined ? '-' : `${v > 0 ? '+' : ''}${v.toFixed(2)}%`)
const cls = (v: number | null | undefined) => (!v ? '' : v > 0 ? 'up' : 'down')

const STATUS: Record<string, string> = {
  WATCHING: '감시 중', PAUSED: '일시정지', ORDERING: '주문 중', ORDERED: '주문 완료', COMPLETED: '완료', EXPIRED: '만료',
  HOLDING: '대기', CANCELED: '취소',
}
const TYPE: Record<string, string> = { SINGLE: '단일', OCO: 'OCO (익절/손절)', OTO: 'OTO (매수 후 매도)' }

function legText(leg: ConditionalLeg | undefined, currency: 'KRW' | 'USD') {
  if (!leg) return '-'
  const side = leg.orderSide === 'SELL' ? '매도' : '매수'
  const trigger = leg.triggerPrice != null ? money(leg.triggerPrice, currency) : leg.targetProfitRate != null ? `수익률 ${leg.targetProfitRate}%` : '-'
  return `${trigger} → ${leg.orderPrice != null ? money(leg.orderPrice, currency) : '시장가'} ${side} · ${STATUS[leg.status] ?? leg.status}`
}

export default function AccountPage() {
  const [summary, setSummary] = useState<AccountSummary | null>(null)
  const [orders, setOrders] = useState<ConditionalOrder[]>([])
  const [orderStatus, setOrderStatus] = useState<'OPEN' | 'CLOSED'>('OPEN')
  const [afterCost, setAfterCost] = useState(true)
  const [oco, setOco] = useState<Holding | null>(null)
  const [error, setError] = useState<{ code: string; message: string } | null>(null)
  const [loading, setLoading] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [s, o] = await Promise.all([accountApi.summary(), accountApi.conditionalOrders(orderStatus)])
      setSummary(s)
      setOrders(o.items)
      setError(null)
    } catch (e) {
      setError({ code: e instanceof ApiError ? e.code : 'error', message: (e as Error).message })
    } finally {
      setLoading(false)
    }
  }, [orderStatus])

  useEffect(() => {
    load()
    const t = window.setInterval(() => document.visibilityState === 'visible' && load(), 30_000)
    return () => window.clearInterval(t)
  }, [load])

  const cancel = async (o: ConditionalOrder) => {
    if (!confirm(`${o.symbol} 조건 주문을 취소할까요?`)) return
    try {
      await accountApi.cancelConditional(o.conditionalOrderId)
      load()
    } catch (e) {
      alert((e as Error).message)
    }
  }

  if (error?.code === 'toss_credentials_required')
    return <div className="placeholder">토스증권 API 키가 필요합니다. <Link to="/settings">설정에서 등록하기 →</Link></div>

  const h = summary?.holdings
  const mv = afterCost ? h?.marketValue.amountAfterCost : h?.marketValue.amount
  const pl = afterCost ? h?.profitLoss.amountAfterCost : h?.profitLoss.amount
  const plPct = afterCost ? h?.profitLoss.rateAfterCostPct : h?.profitLoss.ratePct
  const nameOf = (sym: string) => h?.items.find((i) => i.symbol === sym)?.name ?? sym

  return (
    <div className="account">
      <div className="account-head">
        <h2>계좌</h2>
        <label className="check small">
          <input type="checkbox" checked={afterCost} onChange={(e) => setAfterCost(e.target.checked)} /> 매도 비용(수수료·세금) 차감 후
        </label>
        <div className="spacer" />
        <button onClick={load} disabled={loading}>{loading ? '불러오는 중…' : '새로고침'}</button>
      </div>
      <FeatureGuideBox guide={FEATURE_GUIDES.account} />
      {error && <p className="error">{error.message}</p>}

      {h && summary && (
        <div className="stat-tiles">
          <div className="stat-tile">
            <span className="muted small">총 평가금액</span>
            <b>{money(mv?.krw, 'KRW')}</b>
            {mv?.usd != null && <span className="small">{money(mv.usd, 'USD')}</span>}
          </div>
          <div className="stat-tile">
            <span className="muted small">총 손익</span>
            <b className={cls(pl?.krw)}>{signed(pl?.krw, 'KRW')}</b>
            <span className={`small ${cls(plPct)}`}>{pct(plPct)}{pl?.usd != null && ` · ${signed(pl.usd, 'USD')}`}</span>
          </div>
          <div className="stat-tile">
            <span className="muted small">오늘 손익</span>
            <b className={cls(h.dailyProfitLoss.amount.krw)}>{signed(h.dailyProfitLoss.amount.krw, 'KRW')}</b>
            <span className={`small ${cls(h.dailyProfitLoss.ratePct)}`}>{pct(h.dailyProfitLoss.ratePct)}</span>
          </div>
          <div className="stat-tile">
            <span className="muted small">매수 가능 금액</span>
            <b>{money(summary.buying_power.KRW, 'KRW')}</b>
            <span className="small">{money(summary.buying_power.USD, 'USD')}</span>
          </div>
        </div>
      )}

      <h4>보유 종목 ({h?.items.length ?? 0})</h4>
      <div className="trades-wrap">
        <table className="trades holdings cards">
          <thead>
            <tr>
              <th>종목</th>
              <th>보유</th>
              <th>평균단가</th>
              <th>현재가</th>
              <th>평가금액</th>
              <th>손익</th>
              <th>수익률</th>
              <th>오늘</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {h?.items.map((it) => {
              const p = afterCost ? it.profitLoss.amountAfterCost : it.profitLoss.amount
              const r = afterCost ? it.profitLoss.rateAfterCostPct : it.profitLoss.ratePct
              return (
                <tr key={it.symbol}>
                  <td className="card-title">
                    <b>{it.name}</b> <span className="muted small">{it.symbol}</span>
                  </td>
                  <td data-label="보유">{it.quantity.toLocaleString()}</td>
                  <td data-label="평균단가">{money(it.averagePurchasePrice, it.currency)}</td>
                  <td data-label="현재가">{money(it.lastPrice, it.currency)}</td>
                  <td data-label="평가금액">{money(afterCost ? it.marketValue.amountAfterCost : it.marketValue.amount, it.currency)}</td>
                  <td data-label="손익" className={cls(p)}>{signed(p, it.currency)}</td>
                  <td data-label="수익률" className={cls(r)}>{pct(r)}</td>
                  <td data-label="오늘" className={cls(it.dailyProfitLoss.ratePct)}>{pct(it.dailyProfitLoss.ratePct)}</td>
                  <td className="card-actions"><button className="link" onClick={() => setOco(it)}>익절·손절</button></td>
                </tr>
              )
            })}
            {h && !h.items.length && (
              <tr><td colSpan={9} className="muted">보유 종목이 없습니다.</td></tr>
            )}
          </tbody>
        </table>
      </div>

      <OrdersSection nameOf={nameOf} />

      <div className="account-head">
        <h4>조건 주문</h4>
        <div className="tabs">
          {(['OPEN', 'CLOSED'] as const).map((s) => (
            <button key={s} className={orderStatus === s ? 'active' : ''} onClick={() => setOrderStatus(s)}>
              {s === 'OPEN' ? '진행 중' : '종료'}
            </button>
          ))}
        </div>
        <span className="muted small">토스증권 앱에서 만든 조건 주문도 함께 보입니다.</span>
      </div>
      <div className="trades-wrap">
        <table className="trades left-first cards">
          <thead>
            <tr>
              <th>종목</th>
              <th>유형</th>
              <th>상태</th>
              <th>수량</th>
              <th>조건 1</th>
              <th>조건 2</th>
              <th>만료일</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {orders.map((o) => {
              const currency = o.market === 'KR' ? 'KRW' : 'USD'
              return (
                <tr key={o.conditionalOrderId}>
                  <td className="card-title"><b>{nameOf(o.symbol)}</b> <span className="muted small">{o.symbol}</span></td>
                  <td data-label="유형">{TYPE[o.type] ?? o.type}</td>
                  <td data-label="상태">{STATUS[o.status] ?? o.status}</td>
                  <td data-label="수량">{o.quantity.toLocaleString()}</td>
                  <td data-label="조건 1">{legText(o.first, currency)}</td>
                  <td data-label="조건 2">{legText(o.second, currency)}</td>
                  <td data-label="만료일">{o.expireDate ?? '-'}</td>
                  <td className="card-actions">
                    {(o.status === 'WATCHING' || o.status === 'PAUSED') && (
                      <button className="link danger-link" onClick={() => cancel(o)}>취소</button>
                    )}
                  </td>
                </tr>
              )
            })}
            {!orders.length && (
              <tr><td colSpan={8} className="muted">{orderStatus === 'OPEN' ? '진행 중인' : '종료된'} 조건 주문이 없습니다.</td></tr>
            )}
          </tbody>
        </table>
      </div>
      {oco && <OcoDialog holding={oco} onClose={() => setOco(null)} onDone={load} />}
    </div>
  )
}
