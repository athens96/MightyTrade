import { api } from './client'

export type RequestCategory = 'feature' | 'bug' | 'strategy' | 'other'
export type RequestStatus = 'open' | 'in_review' | 'resolved' | 'closed'

export const REQUEST_CATEGORIES: { id: RequestCategory; label: string; tip: string }[] = [
  { id: 'feature', label: '💡 기능 건의', tip: '더 편리한 화면 구성이나 새로 추가되었으면 하는 기능을 제안합니다.' },
  { id: 'bug', label: '🐞 오류·버그 제보', tip: '화면 표시가 어색하거나 예상과 다르게 동작하는 문제를 알려줍니다.' },
  { id: 'strategy', label: '📈 전략·지표 추가', tip: '차트·퀀트 랩·자동매매 봇에 추가하고 싶은 보조지표나 매매 전략을 요청합니다.' },
  { id: 'other', label: '💬 기타 문의', tip: '계정·사용법 등 기타 궁금한 점이나 의견을 남깁니다.' },
]

export const REQUEST_CATEGORY_LABEL: Record<RequestCategory, string> = {
  feature: '기능 건의',
  bug: '오류·버그 제보',
  strategy: '전략·지표 추가',
  other: '기타 문의',
}

export const REQUEST_STATUS_LABEL: Record<RequestStatus, string> = {
  open: '접수 대기',
  in_review: '검토 중',
  resolved: '반영 완료',
  closed: '보류·종료',
}

export interface UserRequestItem {
  id: string
  category: RequestCategory
  title: string
  content: string
  page: string | null
  app_version: string | null
  status: RequestStatus
  admin_reply: string
  replied_at: string | null
  created_at: string | null
  updated_at: string | null
}

export interface CreateUserRequestInput {
  category: RequestCategory
  title: string
  content: string
  page?: string
  app_version?: string
}

export const requestsApi = {
  listMine: () => api<{ items: UserRequestItem[] }>('/users/me/requests'),
  create: (input: CreateUserRequestInput) =>
    api<{ item: UserRequestItem }>('/users/me/requests', { method: 'POST', json: input }),
}
