export type TossMockupId =
  | 'toss-login-wts'
  | 'toss-open-api-menu'
  | 'toss-copy-keys'
  | 'toss-allowed-ip'
  | 'toss-paste-otp'

export interface TossGuideLink {
  label: string
  url: string
}

export interface TossGuideStep {
  id: string
  shortTitle: string
  title: string
  summary: string
  mockup: TossMockupId
  actions: string[]
  links?: TossGuideLink[]
  tip?: string
  caution?: string
}

export interface TossQuickCard {
  id: string
  stepBadge: string
  title: string
  desc: string
  stepIndex: number
}

export interface TossFaqItem {
  q: string
  a: string
  stepIndex?: number
}

export interface TossSecurityPoint {
  icon: string
  title: string
  desc: string
}

export const TOSS_GUIDE_STEPS: TossGuideStep[] = [
  {
    id: 'toss-1',
    shortTitle: 'PC 웹(WTS) 열기',
    title: '1단계 · 토스증권 PC 웹(WTS) 로그인하기',
    summary: '토스증권 Open API 키 발급과 허용 IP 설정은 스마트폰 앱이 아니라 PC 웹(WTS) 화면에서만 할 수 있습니다.',
    mockup: 'toss-login-wts',
    actions: [
      'PC 브라우저(크롬·사파리·엣지 등)에서 토스증권 PC 웹(tossinvest.com)을 엽니다.',
      '오른쪽 위 [로그인]을 누르고 스마트폰 토스 앱 QR 코드(또는 간편 인증)로 로그인합니다.',
      '본인 명의 토스증권 종합위탁 계좌가 정상 조회되는지 확인합니다.',
    ],
    links: [{ label: '토스증권 PC 웹(WTS) 바로 열기 ↗', url: 'https://www.tossinvest.com' }],
    tip: '스마트폰 토스 앱에서 QR 카메라로 스캔하면 비밀번호를 따로 칠 필요 없이 10초 만에 로그인됩니다.',
    caution: '스마트폰 토스 앱 안에는 Open API 발급 메뉴가 없습니다. 반드시 PC 웹 브라우저에서 진행해 주세요.',
  },
  {
    id: 'toss-2',
    shortTitle: 'Open API 메뉴',
    title: '2단계 · 설정(⚙️) → [Open API] 메뉴 열기 및 신청',
    summary: '토스증권 WTS 안의 설정 메뉴에서 [Open API] 화면으로 들어가 이용 약관에 동의합니다.',
    mockup: 'toss-open-api-menu',
    actions: [
      '토스증권 WTS 화면에서 [설정 (⚙️ 톱니바퀴)] 아이콘을 클릭합니다.',
      '설정 메뉴 목록에서 [Open API] 항목을 선택합니다.',
      '처음 접속했다면 [Open API 이용 신청]을 누르고 약관 동의 및 본인 확인을 마칩니다.',
    ],
    tip: '이미 예전에 Open API를 신청해 두셨다면 바로 키 관리 화면이 보입니다.',
  },
  {
    id: 'toss-3',
    shortTitle: 'ID·Secret 복사',
    title: '3단계 · Client ID와 Client Secret 발급 후 즉시 복사',
    summary: '마이티 트레이드가 내 토스증권 계좌에 안전하게 연결할 수 있는 전용 열쇠 2개를 발급받습니다.',
    mockup: 'toss-copy-keys',
    actions: [
      'Open API 관리 화면에서 [키 발급 (또는 새 키 만들기)] 버튼을 누릅니다.',
      '화면에 표시된 Client ID (예: tsck_live_…) 옆 복사 버튼을 눌러 복사합니다.',
      '함께 표시된 Client Secret 도 놓치지 말고 지금 바로 복사해 둡니다.',
    ],
    caution: 'Client Secret은 발급 직후 단 한 번만 전체가 보이는 경우가 많습니다. 창을 닫기 전에 꼭 복사해 두세요! 분실했다면 WTS에서 새 키로 재발급하면 됩니다.',
    tip: '발급한 키는 절대 카톡·블로그·캡처 이미지 등에 공유하지 마세요.',
  },
  {
    id: 'toss-4',
    shortTitle: '허용 IP 등록(필수)',
    title: '4단계 · [허용 IP 관리]에 이 서비스 서버 IP 등록하기 (필수!)',
    summary: '토스증권은 해킹 방지를 위해 미리 등록된 IP 주소에서 온 요청만 통과시킵니다. 이 단계를 빼먹으면 연결이 거절됩니다.',
    mockup: 'toss-allowed-ip',
    actions: [
      '같은 [설정 → Open API] 화면 아래쪽의 [허용 IP 관리] 섹션을 찾습니다.',
      '아래에 표시된 이 서비스 서버의 공인 IP를 복사해 입력 칸에 붙여넣습니다.',
      '[추가 / 저장] 버튼을 눌러 허용 IP 목록에 정상 등록되었는지 확인합니다.',
    ],
    caution: '허용 IP를 등록하지 않거나 오타가 있으면, 5단계에서 [확인 후 저장]을 눌렀을 때 "403 Forbidden" 또는 토스 인증 실패 오류가 발생합니다!',
    tip: '토스증권 정책상 1개의 API 키로 동시에 2곳에서 접속할 수 없습니다. 같은 키를 내 PC 프로그램과 이 서버에 동시에 켜두면 한쪽 연결이 끊길 수 있습니다.',
  },
  {
    id: 'toss-5',
    shortTitle: '입력 · OTP 검증',
    title: '5단계 · 마이티 트레이드 설정에 입력하고 6자리 OTP 확인!',
    summary: '복사해 둔 2개의 키와 스마트폰 인증 앱의 6자리 일회용 번호(OTP)를 입력해 연결을 완료합니다.',
    mockup: 'toss-paste-otp',
    actions: [
      '아래 [토스 키 입력창으로 바로 이동 →] 버튼을 눌러 설정 화면으로 돌아갑니다.',
      '[Client ID]와 [Client Secret] 칸에 3단계에서 복사한 값을 각각 붙여넣습니다.',
      '스마트폰 OTP 앱(Google Authenticator 등)에 표시된 6자리 숫자를 [OTP 코드] 칸에 입력합니다.',
      '[확인 후 저장]을 누르면 서버가 토스증권에 실제 계좌 연결을 확인한 뒤 암호화해 저장합니다.',
    ],
    tip: '저장에 성공하면 위쪽 [현재 상태]에 마스킹된 Client ID와 내 계좌 번호(accountSeq)가 뜨고, 대시보드·내 계좌·자동매매에서 실시간 시세와 거래가 바로 열립니다!',
  },
]

export const TOSS_QUICK_CARDS: TossQuickCard[] = [
  {
    id: 'keys',
    stepBadge: '1~3단계',
    title: 'WTS에서 API 키 발급',
    desc: 'PC 웹(tossinvest.com) → 설정 → Open API에서 Client ID·Secret을 발급받습니다.',
    stepIndex: 0,
  },
  {
    id: 'ip',
    stepBadge: '4단계 · 필수',
    title: '허용 IP 관리 등록',
    desc: '같은 화면의 허용 IP 관리에 이 서비스 서버의 IP를 꼭 등록해야 연결됩니다.',
    stepIndex: 3,
  },
  {
    id: 'verify',
    stepBadge: '5단계',
    title: '입력 · OTP 검증 저장',
    desc: '키 2개와 6자리 OTP를 입력하면 토스 계좌 연결을 확인한 뒤 이중 암호화해 저장합니다.',
    stepIndex: 4,
  },
]

export const TOSS_SECURITY_POINTS: TossSecurityPoint[] = [
  {
    icon: '🔒',
    title: '브라우저 이중 암호화 전송',
    desc: '입력한 Client ID·Secret·OTP는 내 브라우저 안에서 RSA-3072 + AES-256-GCM으로 먼저 잠긴 뒤 서버로 전달되므로 중간 도청이 불가능합니다.',
  },
  {
    icon: '🗄️',
    title: '서버 암호화 보관 · 원본 미노출',
    desc: '서버 데이터베이스에도 Fernet 암호화로 잠가 보관하며, 저장 이후에는 화면이나 API로 원래 Secret이 다시 노출되지 않습니다.',
  },
  {
    icon: '📱',
    title: '실거래·키 변경 시 OTP 2단계 잠금',
    desc: '키 등록·변경과 실제 돈이 나가는 주문·실거래 봇 전환은 항상 스마트폰 6자리 OTP 본인 확인을 거쳐야만 작동합니다.',
  },
]

export const TOSS_FAQ_ITEMS: TossFaqItem[] = [
  {
    q: '[확인 후 저장]을 눌렀는데 403 또는 인증 실패 오류가 떠요.',
    a: '가장 흔한 원인은 4단계 [허용 IP 관리]에 이 서비스 서버 IP가 빠졌거나 오타가 있는 경우입니다. 토스증권 WTS 허용 IP 목록에 서버 IP가 정확히 들어갔는지 확인한 뒤 다시 눌러보세요.',
    stepIndex: 3,
  },
  {
    q: 'Client Secret을 복사 못 하고 창을 닫아버렸어요.',
    a: '보안상 토스증권에서는 Secret을 발급 직후에만 보여주는 경우가 많습니다. WTS [설정 → Open API]에서 키를 재발급받은 뒤 새 Client ID와 Client Secret으로 등록하시면 됩니다.',
    stepIndex: 2,
  },
  {
    q: '스마트폰 토스 앱에서 Open API 메뉴를 못 찾겠어요.',
    a: 'Open API 발급과 허용 IP 관리는 토스증권 PC 웹(WTS, tossinvest.com) 전용 메뉴입니다. PC 브라우저에서 토스 앱 QR 로그인으로 접속해 주세요.',
    stepIndex: 0,
  },
  {
    q: '연결은 됐는데 실시간 시세나 접속이 자꾸 끊겨요.',
    a: '토스증권 규정상 하나의 API 키로 동시에 두 곳에서 접속할 수 없습니다. 같은 키를 내 PC 프로그램과 운영 서버에 동시에 켜두지 않았는지 확인해 주세요.',
    stepIndex: 3,
  },
]

export type TossFieldAnchor = 'client-id' | 'client-secret' | 'allowed-ip' | 'otp'

export function tossGuideTargetForField(anchor: TossFieldAnchor): number {
  switch (anchor) {
    case 'client-id':
      return 2 // 3단계: Client ID / Secret 복사
    case 'client-secret':
      return 2 // 3단계: Client ID / Secret 복사
    case 'allowed-ip':
      return 3 // 4단계: 허용 IP 등록
    case 'otp':
      return 4 // 5단계: 입력 · OTP 검증
  }
}
