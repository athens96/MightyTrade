import { useCallback, useEffect, useState, type FormEvent } from 'react'
import {
  REQUEST_CATEGORIES,
  REQUEST_CATEGORY_LABEL,
  REQUEST_STATUS_LABEL,
  requestsApi,
  type RequestCategory,
  type UserRequestItem,
} from '../api/requests'
import { APP_VERSION } from '../lib/version'

const formatTime = (iso: string | null) =>
  iso
    ? new Date(iso).toLocaleString('ko-KR', {
        year: '2-digit',
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      })
    : '-'

export default function RequestModal({ currentPage, onClose }: { currentPage: string; onClose: () => void }) {
  const [tab, setTab] = useState<'write' | 'mine'>('write')
  const [category, setCategory] = useState<RequestCategory>('feature')
  const [title, setTitle] = useState('')
  const [content, setContent] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [items, setItems] = useState<UserRequestItem[]>([])
  const [loadingMine, setLoadingMine] = useState(false)

  const loadMine = useCallback(() => {
    setLoadingMine(true)
    requestsApi
      .listMine()
      .then((r) => setItems(r.items))
      .catch((e) => setError((e as Error).message))
      .finally(() => setLoadingMine(false))
  }, [])

  useEffect(() => {
    loadMine()
  }, [loadMine])

  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError('')
    setNotice('')
    try {
      const res = await requestsApi.create({
        category,
        title: title.trim(),
        content: content.trim(),
        page: currentPage,
        app_version: APP_VERSION,
      })
      setItems((prev) => [res.item, ...prev])
      setTitle('')
      setContent('')
      setNotice('요청사항이 관리자에게 안전하게 전달되었습니다. 아래에서 처리 상태와 답변을 확인하실 수 있습니다.')
      setTab('mine')
      window.dispatchEvent(new Event('mt:admin-changed'))
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal req-modal" role="dialog" aria-modal="true" aria-labelledby="req-modal-title">
        <div className="account-head">
          <h3 id="req-modal-title">💬 사용자 요청사항</h3>
          <div className="spacer" />
          <button type="button" title="요청사항 창을 닫습니다" onClick={onClose}>닫기</button>
        </div>

        <div className="tabs">
          <button
            type="button"
            className={tab === 'write' ? 'active' : ''}
            title="기능 건의나 오류 제보를 작성해 관리자에게 전달합니다"
            onClick={() => { setTab('write'); setError('') }}
          >
            ✉️ 새 요청 보내기
          </button>
          <button
            type="button"
            className={tab === 'mine' ? 'active' : ''}
            title="내가 지금까지 보낸 요청사항의 진행 상태와 관리자 답변을 확인합니다"
            onClick={() => { setTab('mine'); setError(''); loadMine() }}
          >
            📋 내가 보낸 요청{items.length ? ` (${items.length})` : ''}
          </button>
        </div>

        <div className="req-privacy small">
          🔒 보내신 요청사항은 <b>관리자만 수신·열람</b>할 수 있으며 다른 사용자에게는 절대 공개되지 않습니다.
        </div>

        {notice && <p className="ok small" style={{ margin: 0 }}>{notice}</p>}
        {error && <p className="error small" style={{ margin: 0 }}>{error}</p>}

        {tab === 'write' ? (
          <form className="form" onSubmit={submit}>
            <div className="tabs bot-cat-tabs">
              {REQUEST_CATEGORIES.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  className={category === c.id ? 'active' : ''}
                  title={c.tip}
                  onClick={() => setCategory(c.id)}
                >
                  {c.label}
                </button>
              ))}
            </div>

            <label title="요청 핵심 내용을 한 줄(2~100자)로 간단히 적어 주세요">
              제목
              <input
                value={title}
                maxLength={100}
                placeholder="예: 퀀트 랩에 일목균형표 전략을 추가해 주세요"
                title="관리자가 한눈에 알아볼 수 있도록 요청 제목(2~100자)을 입력합니다"
                onChange={(e) => setTitle(e.target.value)}
              />
            </label>

            <label title="원하시는 동작이나 불편했던 상황을 편하게 적어 주세요 (5~2,000자)">
              상세 내용
              <textarea
                rows={5}
                value={content}
                maxLength={2000}
                placeholder="어떤 기능이 필요한지, 혹은 어떤 화면에서 불편했는지 자세히 적어 주시면 빠르게 반영하는 데 큰 도움이 됩니다."
                title="구체적인 건의 내용이나 재현 방법을 입력합니다 (5~2,000자)"
                onChange={(e) => setContent(e.target.value)}
              />
            </label>

            <p className="muted small" style={{ margin: 0 }}>
              현재 화면(<code>{currentPage}</code>) · 앱 버전(<code>v{APP_VERSION}</code>) 정보가 함께 첨부됩니다.
            </p>

            <div className="modal-actions" style={{ width: '100%' }}>
              <button type="button" title="작성을 취소하고 창을 닫습니다" onClick={onClose}>취소</button>
              <button
                type="submit"
                className="primary"
                disabled={busy || title.trim().length < 2 || content.trim().length < 5}
                title="작성한 요청사항을 관리자에게 전송합니다"
              >
                {busy ? '전송 중…' : '관리자에게 요청 보내기'}
              </button>
            </div>
          </form>
        ) : (
          <div className="req-list">
            {loadingMine && !items.length && <p className="muted small">불러오는 중…</p>}
            {!loadingMine && !items.length && (
              <p className="muted small">아직 보낸 요청사항이 없습니다. 필요한 기능이나 불편한 점이 있으면 언제든 남겨 주세요.</p>
            )}
            {items.map((r) => (
              <div key={r.id} className="req-card">
                <div className="req-card-head">
                  <span className="cat-badge">{REQUEST_CATEGORY_LABEL[r.category] ?? r.category}</span>
                  <span className={`status-badge ${r.status}`}>{REQUEST_STATUS_LABEL[r.status] ?? r.status}</span>
                  <span className="muted small spacer-r">{formatTime(r.created_at)}</span>
                </div>
                <b className="req-title">{r.title}</b>
                <p className="req-content small">{r.content}</p>
                {r.admin_reply && (
                  <div className="req-reply-box small">
                    <div className="req-reply-head">
                      <b>💬 관리자 답변</b>
                      {r.replied_at && <span className="muted">{formatTime(r.replied_at)}</span>}
                    </div>
                    <p>{r.admin_reply}</p>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
