export type KiwoomMockupId =
  | 'kiwoom-portal-login'
  | 'kiwoom-register-ip'
  | 'kiwoom-copy-keys'
  | 'kiwoom-paste-otp'
  | 'kiwoom-ready-features'

export interface KiwoomGuideLink {
  label: string
  url: string
}

export interface KiwoomGuideStep {
  id: string
  shortTitle: string
  title: string
  summary: string
  mockup: KiwoomMockupId
  actions: string[]
  links?: KiwoomGuideLink[]
  tip?: string
  caution?: string
}

export interface KiwoomQuickCard {
  id: string
  stepBadge: string
  title: string
  desc: string
  stepIndex: number
}

export interface KiwoomFaqItem {
  q: string
  a: string
  stepIndex?: number
}

export interface KiwoomSecurityPoint {
  icon: string
  title: string
  desc: string
}

export const KIWOOM_GUIDE_STEPS: KiwoomGuideStep[] = [
  {
    id: 'kw-1',
    shortTitle: '포털 접속·신청',
    title: '1단계 · 키움 REST API 공식 포털 접속 및 서비스 신청',
    summary: '키움 REST API는 예전 구형 Open API+와 달리 윈도우 프로그램이나 공인인증서 없이 웹에서 바로 신청합니다.',
    mockup: 'kiwoom-portal-login',
    actions: [
      '아래 링크로 키움증권 공식 REST API 포털(openapi.kiwoom.com)을 열고 로그인합니다.',
      '[서비스 이용신청] 메뉴에서 약관에 동의하고 연동할 키움증권 계좌를 선택합니다.',
      '먼저 연습해 보고 싶다면 키움 홈페이지 [모의투자 → 상시모의투자]를 신청해 모의투자 전용 키를 받을 수도 있습니다.',
    ],
    links: [{ label: '키움 REST API 공식 포털 열기 ↗', url: 'https://openapi.kiwoom.com' }],
    tip: '실전투자 키(api.kiwoom.com)와 모의투자 키(mockapi.kiwoom.com)는 서로 별도로 발급됩니다. 처음이라면 모의투자 키로 안전하게 연습해 보세요!',
  },
  {
    id: 'kw-2',
    shortTitle: '허용 IP 등록(필수)',
    title: '2단계 · 키움 포털에 이 서비스 서버 IP 등록하기 (최대 10개)',
    summary: '키움 REST API는 보안을 위해 허용 IP를 먼저 등록해야 App Key·Secret Key 발급과 API 호출이 통과됩니다.',
    mockup: 'kiwoom-register-ip',
    actions: [
      '키움 REST API 포털의 [계좌/앱 관리 → IP 등록 관리] 화면으로 이동합니다.',
      '아래 [📋 이 서버 IP 복사] 버튼으로 서비스 서버 공인 IP를 복사해 입력 칸에 붙여넣습니다.',
      '[추가 / 저장]을 눌러 허용 IP 목록에 등록되었는지 확인합니다.',
    ],
    caution: '허용 IP가 빠져 있으면 토큰 발급과 주문·시세 조회가 보안 정책에 의해 즉시 차단됩니다.',
    tip: '키움 REST API는 최대 10개의 IP를 등록할 수 있습니다.',
  },
  {
    id: 'kw-3',
    shortTitle: 'App·Secret 복사',
    title: '3단계 · App Key와 Secret Key 발급받고 즉시 복사하기',
    summary: '마이티 트레이드가 키움 계좌와 안전하게 통신할 수 있는 전용 열쇠 2개(App Key, Secret Key)를 복사합니다.',
    mockup: 'kiwoom-copy-keys',
    actions: [
      '키움 REST API 포털의 [앱 키 관리]에서 사용할 환경(실전투자 또는 모의투자)을 확인합니다.',
      '화면에 표시된 App Key와 Secret Key를 각각 복사합니다.',
      '(선택) 본인 확인용으로 계좌번호 8~10자리를 메모해 두면 설정 화면에 함께 기록할 수 있습니다.',
    ],
    caution: '실전투자용 키를 넣고 [모의투자 서버]를 켜거나, 반대로 모의투자 키를 넣고 [실전투자]를 고르면 인증에 실패합니다. 발급받은 종류에 맞게 맞춰 주세요!',
    tip: 'Secret Key는 계좌 비밀번호와 같으므로 외부 메신저나 화면 캡처로 공유하지 마세요.',
  },
  {
    id: 'kw-4',
    shortTitle: '입력 · OTP 검증',
    title: '4단계 · 마이티 트레이드 설정에 입력하고 6자리 OTP 확인!',
    summary: '실전/모의 구분을 고르고 복사한 App Key·Secret Key와 스마트폰 6자리 OTP를 입력해 연결을 검증합니다.',
    mockup: 'kiwoom-paste-otp',
    actions: [
      '아래 [키움 키 입력창으로 바로 이동 →] 버튼을 눌러 설정 화면으로 돌아갑니다.',
      '[실전투자 / 공식 모의투자] 스위치를 발급받은 키 종류에 맞게 선택합니다.',
      'App Key, Secret Key, 스마트폰 인증 앱의 6자리 OTP 숫자를 입력하고 [확인 후 저장]을 누릅니다.',
      '서버가 키움 OAuth2(au10001) 토큰 발급을 바로 테스트한 뒤 암호화해 안전하게 저장합니다.',
    ],
    tip: '토스증권과 키움증권을 둘 다 등록해 두고, 설정 상단에서 버튼 한 번으로 [사용 중인 증권사]를 자유롭게 전환할 수 있습니다!',
  },
  {
    id: 'kw-5',
    shortTitle: '실시간 · 조건검색',
    title: '5단계 · 연결 완료! 키움 실시간 호가 & 영웅문 조건검색 활용',
    summary: '키움 계좌가 연결되면 대시보드·내 계좌·6종 자동매매 봇뿐 아니라 키움 전용 실시간 호가와 영웅문 조건검색식도 바로 쓸 수 있습니다.',
    mockup: 'kiwoom-ready-features',
    actions: [
      '대시보드에서 국내·미국 차트와 키움 WebSocket 실시간 체결·10단계 호가를 바로 확인합니다.',
      '[퀀트 랩 → 종목 발굴] 탭의 [🟣 영웅문 조건검색식 불러오기]에서 내 HTS 조건검색식을 바로 조회해 백테스트·자동매매 봇으로 연결할 수 있습니다.',
      '자동매매 봇의 손절·익절·트레일링 보호막과 서킷브레이커·실시간 텔레그램/디스코드/슬랙 알림이 그대로 작동합니다.',
    ],
    caution: '키움증권 정책상 실전 서버에 3개월 이상 API 접속 기록이 없으면 매월 첫 영업일에 API 권한이 자동 해지될 수 있습니다.',
  },
]

export const KIWOOM_QUICK_CARDS: KiwoomQuickCard[] = [
  {
    id: 'portal-ip',
    stepBadge: '1~2단계 · 필수',
    title: '포털 신청 & 허용 IP 등록',
    desc: 'openapi.kiwoom.com에서 이용 신청 후 허용 IP(최대 10개)에 이 서버 IP를 등록합니다.',
    stepIndex: 0,
  },
  {
    id: 'app-keys',
    stepBadge: '3~4단계',
    title: '실전/모의 키 발급 · OTP 저장',
    desc: '실전 또는 공식 모의투자 App Key·Secret Key를 복사해 6자리 OTP로 안전하게 저장합니다.',
    stepIndex: 2,
  },
  {
    id: 'features',
    stepBadge: '5단계 · 키움 특화',
    title: '실시간 WS & 영웅문 조건검색',
    desc: '실시간 호가·체결, 국내·미국 자동매매 봇, 퀀트 랩 영웅문 조건검색식 연동까지 바로 씁니다.',
    stepIndex: 4,
  },
]

export const KIWOOM_SECURITY_POINTS: KiwoomSecurityPoint[] = [
  {
    icon: '🔒',
    title: '브라우저 이중 암호화 전송',
    desc: '입력한 App Key·Secret Key·OTP는 브라우저 안에서 RSA-3072 + AES-256-GCM으로 먼저 잠긴 뒤 전송됩니다.',
  },
  {
    icon: '🧪',
    title: '증권사 공식 모의투자 지원',
    desc: '실제 돈을 쓰기 전에 키움 공식 모의투자 서버(mockapi.kiwoom.com)로 실제 주문·체결 흐름을 100% 연습할 수 있습니다.',
  },
  {
    icon: '🚦',
    title: '자동 유량 제어 (Rate Limiter)',
    desc: '키움 REST API 초당 호출 제한(실전 초당 5회·모의 제한)을 넘지 않도록 Rust 서버가 자동으로 요청 간격을 조절합니다.',
  },
]

export const KIWOOM_FAQ_ITEMS: KiwoomFaqItem[] = [
  {
    q: '토스증권이랑 키움증권을 둘 다 등록해도 되나요?',
    a: '네! 두 증권사 키를 모두 안전하게 보관해 두고, 설정 맨 위 [현재 사용 중인 증권사]에서 클릭 한 번으로 토스 ↔ 키움을 전환할 수 있습니다.',
    stepIndex: 3,
  },
  {
    q: '키움에서는 계좌 화면의 [익절·손절 OCO]가 왜 다르게 동작하나요?',
    a: '키움 REST API는 증권사 서버측 OCO 조건주문을 직접 제공하지 않습니다. 대신 마이티 트레이드의 자동매매 봇 손절·익절·트레일링 보호막과 서킷브레이커가 시세를 감시하며 안전하게 보호해 줍니다.',
    stepIndex: 4,
  },
  {
    q: 'App Key가 맞는데 인증 실패 오류가 나요.',
    a: '① 2단계 [허용 IP 관리]에 이 서비스 서버 IP가 등록되었는지, ② 발급받은 키가 [실전투자]인지 [모의투자]인지 스위치와 일치하는지 확인해 주세요.',
    stepIndex: 1,
  },
  {
    q: '영웅문 조건검색식이 목록에 안 떠요.',
    a: '조건검색식 자체는 PC 영웅문4 HTS에서 먼저 작성해 [서버 저장]을 해 두셔야 합니다. 저장해 둔 조건식은 [퀀트 랩 → 종목 발굴]에서 바로 불러올 수 있습니다.',
    stepIndex: 4,
  },
]

export type KiwoomFieldAnchor = 'appkey' | 'secretkey' | 'allowed-ip' | 'mock-mode' | 'otp'

export function kiwoomGuideTargetForField(anchor: KiwoomFieldAnchor): number {
  switch (anchor) {
    case 'allowed-ip':
      return 1 // 2단계: 허용 IP 등록
    case 'appkey':
    case 'secretkey':
      return 2 // 3단계: App Key / Secret Key 복사
    case 'mock-mode':
    case 'otp':
      return 3 // 4단계: 입력 · OTP 검증
  }
}
