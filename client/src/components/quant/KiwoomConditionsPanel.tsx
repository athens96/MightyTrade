import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { PREFILL_KEY } from '../../api/autotrade'
import {
  brokerApi,
  type KiwoomConditionItem,
  type KiwoomConditionMatch,
} from '../../api/broker'
import type { PortfolioAsset } from '../../api/quant'
import { investingUrl } from '../../lib/externalLinks'
import { priceFormatter } from '../chart/format'

interface Props {
  onBacktest: (symbol: string, strategy: string) => void
  onPortfolio: (assets: PortfolioAsset[], modelId: string) => void
}

const krw = priceFormatter('KR')

export default function KiwoomConditionsPanel({ onBacktest, onPortfolio }: Props) {
  const navigate = useNavigate()
  const [kiwoomReady, setKiwoomReady] = useState<boolean | null>(null)
  const [conditions, setConditions] = useState<KiwoomConditionItem[]>([])
  const [selectedSeq, setSelectedSeq] = useState<string>('')
  const [matches, setMatches] = useState<KiwoomConditionMatch[]>([])
  const [busyList, setBusyList] = useState(false)
  const [busyMatch, setBusyMatch] = useState(false)
  const [error, setError] = useState('')
  const [loadedOnce, setLoadedOnce] = useState(false)

  useEffect(() => {
    brokerApi
      .kiwoomStatus()
      .then((s) => setKiwoomReady(Boolean(s.configured)))
      .catch(() => setKiwoomReady(false))
  }, [])

  const loadList = async () => {
    setBusyList(true)
    setError('')
    try {
      const r = await brokerApi.kiwoomConditions()
      setConditions(r.items)
      setLoadedOnce(true)
      if (r.items.length === 0) {
        setMatches([])
        setSelectedSeq('')
      }
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusyList(false)
    }
  }

  const runCondition = async (seq: string) => {
    setSelectedSeq(seq)
    setBusyMatch(true)
    setError('')
    try {
      const r = await brokerApi.kiwoomConditionMatches(seq)
      setMatches(r.items)
    } catch (e) {
      setError((e as Error).message)
      setMatches([])
    } finally {
      setBusyMatch(false)
    }
  }

  const sendToBot = (row: KiwoomConditionMatch) => {
    sessionStorage.setItem(
      PREFILL_KEY,
      JSON.stringify({
        kind: 'single',
        market: 'KR',
        symbol: row.symbol,
        strategy: 'ma_crossover',
      }),
    )
    navigate('/auto')
  }

  const sendTopToPortfolio = () => {
    const picks = matches.slice(0, Math.min(5, matches.length))
    if (picks.length < 2) return
    const eq = Number((100 / picks.length).toFixed(1))
    onPortfolio(
      picks.map((m) => ({
        symbol: m.symbol,
        name: m.name,
        role: 'offensive',
        weight: eq,
      })),
      'equal_weight',
    )
  }

  if (kiwoomReady === false) {
    return (
      <details className="guide-box" style={{ marginBottom: 12 }}>
        <summary>
          <span>🟣 키움증권 영웅문 사용자 조건검색식 연동 (키움 API 등록 시 활성화)</span>
        </summary>
        <div className="guide-body small">
          <p>
            키움증권 REST API를 연결하면 PC 영웅문4 HTS에 서버 저장해 둔 <b>내 조건검색식</b>을 불러와
            포착된 종목을 바로 백테스트·포트폴리오·자동매매 봇으로 연결할 수 있습니다.{' '}
            <Link to="/settings">설정에서 키움 API 연결하기 →</Link>
          </p>
        </div>
      </details>
    )
  }

  if (kiwoomReady === null) return null

  return (
    <section className="panel" style={{ marginBottom: 14 }}>
      <div className="ng-panel-head">
        <h3>🟣 키움 영웅문 조건검색식 불러오기 (국내)</h3>
        <div className="ng-panel-head-btns">
          <button
            type="button"
            className="primary small"
            disabled={busyList}
            title="내 영웅문4 HTS에 서버 저장해 둔 조건검색식 목록을 키움 WebSocket으로 불러옵니다"
            onClick={() => void loadList()}
          >
            {busyList ? '불러오는 중…' : loadedOnce ? '목록 새로고침' : '내 조건검색식 불러오기'}
          </button>
          {matches.length >= 2 && (
            <button
              type="button"
              className="small"
              title="포착된 상위 5종목을 포트폴리오 탭에 바로 담아 분산 백테스트를 실행합니다"
              onClick={sendTopToPortfolio}
            >
              상위 {Math.min(5, matches.length)}종 포트폴리오 →
            </button>
          )}
        </div>
      </div>
      <p className="muted small" style={{ marginTop: 0 }}>
        PC 영웅문4 HTS에서 작성해 서버에 저장한 조건식을 누르면 지금 조건을 충족하는 국내 종목을 즉시 불러옵니다.
      </p>

      {error && <p className="error small">{error}</p>}

      {conditions.length > 0 && (
        <div className="ng-tabs" style={{ marginBottom: 10 }}>
          {conditions.map((c) => (
            <button
              key={c.seq}
              type="button"
              className={selectedSeq === c.seq ? 'active' : ''}
              disabled={busyMatch}
              title={`조건식 #${c.seq} "${c.name}" 충족 종목을 조회합니다`}
              onClick={() => void runCondition(c.seq)}
            >
              #{c.seq} {c.name}
            </button>
          ))}
        </div>
      )}

      {loadedOnce && conditions.length === 0 && !error && (
        <p className="muted small">서버에 저장된 영웅문 조건검색식이 없습니다. 영웅문4 HTS에서 조건식을 서버 저장한 뒤 다시 눌러보세요.</p>
      )}

      {busyMatch && <p className="muted small">조건 충족 종목 조회 중…</p>}

      {!busyMatch && selectedSeq && matches.length === 0 && !error && (
        <p className="muted small">현재 이 조건식을 충족하는 종목이 없습니다.</p>
      )}

      {matches.length > 0 && (
        <div className="trades-wrap">
          <table className="trades left-first cards">
            <thead>
              <tr>
                <th>종목</th>
                <th>현재가</th>
                <th>등락률</th>
                <th>거래량</th>
                <th>바로 연결</th>
              </tr>
            </thead>
            <tbody>
              {matches.map((m) => {
                const cls = m.change_pct > 0 ? 'up' : m.change_pct < 0 ? 'down' : ''
                return (
                  <tr key={m.symbol}>
                    <td className="card-title" data-label="종목">
                      <b>{m.name}</b> <span className="muted small">{m.symbol}</span>{' '}
                      <a
                        className="ext-link"
                        href={investingUrl('KR', m.symbol)}
                        target="_blank"
                        rel="noopener noreferrer"
                        title="Investing.com에서 기업 정보·뉴스 보기"
                      >
                        ↗
                      </a>
                    </td>
                    <td data-label="현재가">{m.price > 0 ? `${krw(m.price)}원` : '-'}</td>
                    <td data-label="등락률" className={cls}>
                      {m.change_pct > 0 ? '+' : ''}{m.change_pct.toFixed(2)}%
                    </td>
                    <td data-label="거래량">{m.volume > 0 ? m.volume.toLocaleString() : '-'}</td>
                    <td className="card-actions" data-label="바로 연결">
                      <div className="row-btns">
                        <button
                          type="button"
                          onClick={() => onBacktest(m.symbol, 'ma_crossover')}
                        >
                          백테스트
                        </button>
                        <button
                          type="button"
                          className="primary"
                          onClick={() => sendToBot(m)}
                        >
                          봇 만들기
                        </button>
                      </div>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  )
}
