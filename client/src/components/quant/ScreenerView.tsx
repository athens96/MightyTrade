import { useNavigate } from 'react-router-dom'
import { PREFILL_KEY } from '../../api/autotrade'
import type { PortfolioAsset, ScreenerPresetMeta, ScreenerResult, ScreenerRow } from '../../api/quant'
import { investingUrl } from '../../lib/externalLinks'
import { priceFormatter } from '../chart/format'

const pct = (v: number | null | undefined, digits = 1, signed = true) => {
  if (v == null || Number.isNaN(v)) return '-'
  const sign = signed && v > 0 ? '+' : ''
  return `${sign}${v.toFixed(digits)}%`
}

const num = (v: number | null | undefined, suffix = '', digits = 1) =>
  v == null || Number.isNaN(v) ? '-' : `${v.toFixed(digits)}${suffix}`

const flowStr = (v: number | null | undefined) => {
  if (v == null || Number.isNaN(v)) return '-'
  const sign = v > 0 ? '+' : ''
  if (Math.abs(v) >= 10_000) return `${sign}${(v / 10_000).toFixed(1)}만주`
  return `${sign}${Math.round(v).toLocaleString()}주`
}

const cls = (v: number | null | undefined) => (v == null ? '' : v > 0 ? 'up' : v < 0 ? 'down' : '')

const SOURCE_LABELS: Record<string, string> = {
  yahoo: '미국 재무: Yahoo Finance',
  naver: '국내 재무: 네이버 금융',
  mock: '연습용 가상 데이터',
  unavailable: '재무 데이터 일시 지연',
}

interface Props {
  result: ScreenerResult
  preset?: ScreenerPresetMeta
  onBacktest: (symbol: string, strategy: string) => void
  onPortfolio: (assets: PortfolioAsset[], modelId: string) => void
}

export default function ScreenerView({ result, preset, onBacktest, onPortfolio }: Props) {
  const navigate = useNavigate()
  const money = priceFormatter(result.market)

  const buildTopPortfolio = (count = 5): { assets: PortfolioAsset[]; modelId: string } => {
    const picks = result.items.slice(0, Math.min(count, result.items.length))
    const modelId = result.recommended_portfolio || 'equal_weight'
    const eq = picks.length ? Number((100 / picks.length).toFixed(1)) : 20
    const assets: PortfolioAsset[] = picks.map((r) => ({
      symbol: r.symbol,
      name: r.name,
      role: 'offensive',
      weight: eq,
    }))
    // Sector rotation requires at least 1 defensive parking ETF
    if (modelId === 'sector_rotation') {
      const defSym = result.market === 'KR' ? '153130' : 'SHY'
      const defName = result.market === 'KR' ? '단기채권 (방어)' : '단기국채 (방어)'
      if (!assets.some((a) => a.symbol === defSym)) {
        assets.push({ symbol: defSym, name: defName, role: 'defensive', weight: 0 })
      }
    }
    return { assets, modelId }
  }

  const sendToBot = (row: ScreenerRow) => {
    sessionStorage.setItem(
      PREFILL_KEY,
      JSON.stringify({
        kind: 'single',
        market: result.market,
        symbol: row.symbol,
        strategy: result.strategy,
      }),
    )
    navigate('/auto')
  }

  const sendTopToRebalBot = () => {
    const { assets, modelId } = buildTopPortfolio(5)
    if (assets.length < 2) return
    sessionStorage.setItem(
      PREFILL_KEY,
      JSON.stringify({
        kind: 'rebalance',
        market: result.market,
        model: modelId,
        assets,
        rebalance: 'monthly',
      }),
    )
    navigate('/auto')
  }

  const p = result.preset
  const isFund = p === 'magic_formula' || p === 'multifactor'
  const isDiv = p === 'dividend_lowvol'
  const isFlow = p === 'smart_money'
  const isFit = p === 'strategy_fit'
  const isOversold = p === 'oversold_bounce'

  return (
    <>
      <div className="result-head">
        <div className="account-head">
          <h3>{preset?.name ?? result.preset} · 종목 발굴 결과</h3>
          <div className="spacer" />
          {result.items.length >= 2 && (
            <div className="screener-actions">
              <button
                type="button"
                title="발굴된 상위 종목들을 포트폴리오 탭에 자동으로 담아 분산 투자 성과를 바로 백테스트합니다"
                onClick={() => {
                  const { assets, modelId } = buildTopPortfolio(5)
                  onPortfolio(assets, modelId)
                }}
              >
                상위 {Math.min(5, result.items.length)}종 포트폴리오 실험 →
              </button>
              <button
                type="button"
                className="primary"
                title="발굴된 상위 종목들을 묶어 정기적으로 비중을 맞추는 자동매매 리밸런싱 봇 생성 창을 엽니다 (모의 모드 시작)"
                onClick={sendTopToRebalBot}
              >
                상위 {Math.min(5, result.items.length)}종 리밸런싱 봇 →
              </button>
            </div>
          )}
        </div>
        <span className="muted small">
          스캔 {result.scanned}종목 중 <b>{result.passed}종목</b> 분석 완료
          {result.matched_count != null && <> · <b className="up">조건 충족 {result.matched_count}종목</b></>}
          {' '}(상위 {result.items.length}개 표시 · {result.elapsed_sec}초)
          {result.skipped > 0 && ` · 데이터 부족 ${result.skipped}개 제외`}
          {result.excluded_warnings.length > 0 && ` · 위험 종목 ${result.excluded_warnings.length}개 자동 제외`}
          {result.data_source && ` · ${result.data_source}`}
          {result.fundamentals_source && ` · ${SOURCE_LABELS[result.fundamentals_source] ?? result.fundamentals_source}`}
        </span>
      </div>

      {result.notices.map((n, i) => (
        <p key={i} className="warn small">{n}</p>
      ))}

      {result.items.length === 0 ? (
        <div className="placeholder muted">조건에 맞는 종목이 없습니다. 다른 후보 묶음이나 발굴 방식을 선택해 보세요.</div>
      ) : (
        <div className="trades-wrap">
          <table className="trades left-first cards">
            <thead>
              <tr>
                <th>순위</th>
                <th>종목</th>
                <th>현재가</th>
                <th>종합 점수</th>
                {isOversold && (
                  <>
                    <th>200일선</th>
                    <th>RSI(2)</th>
                    <th>하루위치(IBS)</th>
                    <th>1개월</th>
                  </>
                )}
                {isFlow && (
                  <>
                    <th>외국인(5일)</th>
                    <th>기관(5일)</th>
                    <th>외국인(20일)</th>
                    <th>3개월</th>
                  </>
                )}
                {isFit && (
                  <>
                    <th>2년 수익률</th>
                    <th>최대 낙폭</th>
                    <th>승률</th>
                    <th>매매수</th>
                  </>
                )}
                {isFund && (
                  <>
                    <th>PER</th>
                    <th>PBR</th>
                    <th>ROE</th>
                    <th>6개월</th>
                  </>
                )}
                {isDiv && (
                  <>
                    <th>연 배당률</th>
                    <th>출렁임(연)</th>
                    <th>1년 수익</th>
                    <th>200일선</th>
                  </>
                )}
                {!isOversold && !isFlow && !isFit && !isFund && !isDiv && (
                  <>
                    <th>3개월</th>
                    <th>6개월</th>
                    <th>출렁임(연)</th>
                    <th>52주 고점대비</th>
                  </>
                )}
                <th>선정 이유</th>
                <th>바로 연결</th>
              </tr>
            </thead>
            <tbody>
              {result.items.map((r) => (
                <tr key={r.symbol}>
                  <td data-label="순위"><b>{r.rank}</b></td>
                  <td className="card-title" data-label="종목">
                    <b>{r.name}</b>{' '}
                    <span className="muted small">{r.symbol}</span>{' '}
                    <a
                      className="ext-link"
                      href={investingUrl(result.market, r.symbol)}
                      target="_blank"
                      rel="noopener noreferrer"
                      title="Investing.com에서 기업 정보·뉴스 보기"
                    >
                      ↗
                    </a>
                    {r.matched && r.match_label && (
                      <span className="match-badge">{r.match_label}</span>
                    )}
                    {r.warnings.length > 0 && (
                      <span className="warn-badge" title={r.warnings.join(', ')}>⚠ {r.warnings[0]}</span>
                    )}
                  </td>
                  <td data-label="현재가">
                    {money(r.price)}{' '}
                    <span className={`small ${cls(r.change_1d_pct)}`}>{pct(r.change_1d_pct)}</span>
                  </td>
                  <td data-label="종합 점수">
                    <span className={`score-pill ${r.score >= 75 ? 'high' : r.score >= 50 ? 'mid' : 'low'}`}>
                      {r.score.toFixed(0)}점
                    </span>
                  </td>

                  {isOversold && (
                    <>
                      <td data-label="200일선" className={r.above_ma200 ? 'up' : 'down'}>{r.above_ma200 ? '위 (상승)' : '아래'}</td>
                      <td data-label="RSI(2)">{num(r.rsi2)}</td>
                      <td data-label="하루위치(IBS)">{num(r.ibs, '', 2)}</td>
                      <td data-label="1개월" className={cls(r.ret_1m_pct)}>{pct(r.ret_1m_pct)}</td>
                    </>
                  )}
                  {isFlow && (
                    <>
                      <td data-label="외국인(5일)" className={cls(r.flow_5d_for)}>{flowStr(r.flow_5d_for)}</td>
                      <td data-label="기관(5일)" className={cls(r.flow_5d_inst)}>{flowStr(r.flow_5d_inst)}</td>
                      <td data-label="외국인(20일)" className={cls(r.flow_20d_for)}>{flowStr(r.flow_20d_for)}</td>
                      <td data-label="3개월" className={cls(r.ret_3m_pct)}>{pct(r.ret_3m_pct)}</td>
                    </>
                  )}
                  {isFit && (
                    <>
                      <td data-label="2년 수익률" className={cls(r.fit_return_pct)}>{pct(r.fit_return_pct)}</td>
                      <td data-label="최대 낙폭" className="down">{pct(r.fit_mdd_pct, 1, false)}</td>
                      <td data-label="승률">{pct(r.fit_win_rate, 0, false)}</td>
                      <td data-label="매매수">{r.fit_trades ?? 0}회</td>
                    </>
                  )}
                  {isFund && (
                    <>
                      <td data-label="PER">{num(r.per, '배')}</td>
                      <td data-label="PBR">{num(r.pbr, '배', 2)}</td>
                      <td data-label="ROE" className={cls(r.roe)}>{pct(r.roe, 1, false)}</td>
                      <td data-label="6개월" className={cls(r.ret_6m_pct)}>{pct(r.ret_6m_pct)}</td>
                    </>
                  )}
                  {isDiv && (
                    <>
                      <td data-label="연 배당률" className="up">{pct(r.dividend_yield, 2, false)}</td>
                      <td data-label="출렁임(연)">{pct(r.vol_60d_pct, 1, false)}</td>
                      <td data-label="1년 수익" className={cls(r.ret_12m_pct)}>{pct(r.ret_12m_pct)}</td>
                      <td data-label="200일선" className={r.above_ma200 ? 'up' : 'down'}>{r.above_ma200 ? '위' : '아래'}</td>
                    </>
                  )}
                  {!isOversold && !isFlow && !isFit && !isFund && !isDiv && (
                    <>
                      <td data-label="3개월" className={cls(r.ret_3m_pct)}>{pct(r.ret_3m_pct)}</td>
                      <td data-label="6개월" className={cls(r.ret_6m_pct)}>{pct(r.ret_6m_pct)}</td>
                      <td data-label="출렁임(연)">{pct(r.vol_60d_pct, 1, false)}</td>
                      <td data-label="52주 고점대비">{pct(r.near_52w_pct, 0, false)}</td>
                    </>
                  )}

                  <td data-label="선정 이유" className="small reason-cell">
                    <div className="reason-tags">
                      {r.reasons.map((txt, idx) => (
                        <span key={idx} className="reason-tag">{txt}</span>
                      ))}
                    </div>
                  </td>
                  <td className="card-actions" data-label="바로 연결">
                    <div className="row-btns">
                      <button
                        type="button"
                        title={`${r.name} 종목을 추천 전략으로 백테스트해 과거 수익률과 매수·매도 타점을 확인합니다`}
                        onClick={() => onBacktest(r.symbol, result.strategy)}
                      >
                        백테스트
                      </button>
                      <button
                        type="button"
                        className="primary"
                        title={`${r.name} 종목으로 자동매매 봇 생성 창을 엽니다 (안전하게 모의 모드·꺼진 상태로 시작)`}
                        onClick={() => sendToBot(r)}
                      >
                        봇 만들기
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}
