export type NotifyChannelId = 'telegram' | 'discord' | 'slack'

export type GuideMockupId =
  | 'tg-search-botfather'
  | 'tg-newbot-token'
  | 'tg-start-mybot'
  | 'tg-get-chatid'
  | 'tg-paste-settings'
  | 'dc-create-server'
  | 'dc-channel-integrations'
  | 'dc-copy-webhook'
  | 'dc-paste-settings'
  | 'sl-create-app'
  | 'sl-app-name'
  | 'sl-enable-webhooks'
  | 'sl-add-channel-copy'
  | 'sl-paste-settings'

export interface GuideCopySnippet {
  label: string
  value: string
}

export interface GuideExternalLink {
  label: string
  url: string
}

export interface NotifyGuideStep {
  id: string
  shortTitle: string
  title: string
  summary: string
  mockup: GuideMockupId
  actions: string[]
  copySnippets?: GuideCopySnippet[]
  links?: GuideExternalLink[]
  tip?: string
  caution?: string
}

export interface NotifyChannelGuide {
  id: NotifyChannelId
  name: string
  emoji: string
  badge: string
  duration: string
  difficulty: string
  bestFor: string
  neededFields: string[]
  steps: NotifyGuideStep[]
}

export interface SampleAlertPreview {
  kind: string
  badge: string
  title: string
  lines: string[]
  note: string
}

export const SAMPLE_ALERT_PREVIEWS: SampleAlertPreview[] = [
  {
    kind: 'order',
    badge: '주문·승인 대기',
    title: '✋ [Mighty Trade · 반자동] 스마트 적립 · 005930 (KR)',
    lines: [
      '✋ [승인 대기] 005930 매수 7주 — 화면에서 [1클릭 승인] 시 실제 주문 전송',
    ],
    note: '반자동(승인 후 주문) 봇이 신호를 포착하면 폰으로 바로 알려줍니다.',
  },
  {
    kind: 'fill',
    badge: '체결 완료',
    title: '✅ [Mighty Trade · 실거래] 터틀 트레이딩 · 005930 (KR)',
    lines: [
      '매수 체결 15주 @ 70,200.00',
    ],
    note: '실제 토스증권 계좌에서 체결이 확인되는 즉시 단가와 수량을 보내줍니다.',
  },
  {
    kind: 'halt',
    badge: '긴급 보호막',
    title: '🛡️ [Mighty Trade · 실거래] 변동성 돌파 · NVDA (US)',
    lines: [
      '🛑 서킷브레이커 발동 — 봇 자동 일시정지: 최대 허용 낙폭 도달 (-10.2% / 한도 -10%)',
    ],
    note: '비상 정지나 서킷브레이커가 걸리면 놓치지 않도록 즉시 발송됩니다.',
  },
  {
    kind: 'briefing',
    badge: '장 마감 일일 브리핑',
    title: '📊 [Mighty Trade] 국내장 마감 봇 성적표',
    lines: [
      '📅 2026-11-23 국내(KRX) 장 마감 일일 브리핑 (가동 봇 3개)',
      '합산 성과: +184,500원 (+1.85%)',
      '• [실거래] 올웨더 리밸런싱 (KR): +1.42% (4종목 보유)',
      '• [반자동] 스마트 적립 · 005930: +2.35% (28주 보유)',
      '• [모의] 순환매 · 상승 추세 (KR): +1.90% (3종목 보유)',
    ],
    note: '국내·미국 정규장이 끝난 뒤 하루 1번, 오늘 수익과 보유 현황을 깔끔하게 요약해 드립니다.',
  },
]

export const NOTIFY_CHANNEL_GUIDES: Record<NotifyChannelId, NotifyChannelGuide> = {
  telegram: {
    id: 'telegram',
    name: '텔레그램',
    emoji: '✈️',
    badge: '추천 · 폰 알림 최강',
    duration: '약 2분',
    difficulty: '보통 (5단계)',
    bestFor: '스마트폰 푸시 알림이 가장 빠르고 깔끔합니다. 반자동 승인 대기·체결 알림을 바로 확인하기 좋습니다.',
    neededFields: ['텔레그램 Bot Token', '텔레그램 Chat ID'],
    steps: [
      {
        id: 'tg-1',
        shortTitle: '@BotFather 열기',
        title: '1단계 · 텔레그램에서 공식 @BotFather 찾기',
        summary: '텔레그램에서 나만의 알림 로봇(봇)을 만들어 주는 공식 도우미 @BotFather를 찾아 대화를 시작합니다.',
        mockup: 'tg-search-botfather',
        actions: [
          '텔레그램 앱(폰 또는 PC)을 열고 위쪽 검색창에 @BotFather 를 입력합니다.',
          '이름 옆에 파란 공식 인증 체크(✓)가 붙은 BotFather를 누릅니다.',
          '화면 아래쪽 [시작 (START)] 버튼을 눌러 대화를 엽니다.',
        ],
        copySnippets: [{ label: '검색어 복사', value: '@BotFather' }],
        links: [{ label: '텔레그램 @BotFather 바로 열기 ↗', url: 'https://t.me/BotFather' }],
        caution: '비슷한 이름의 가짜 봇이 많습니다. 반드시 파란 체크 표시(✓)와 아이디가 정확히 @BotFather 인지 확인하세요.',
      },
      {
        id: 'tg-2',
        shortTitle: '봇 토큰 발급',
        title: '2단계 · /newbot 으로 알림 봇 만들고 Token 복사',
        summary: 'BotFather에게 새 봇을 만들어 달라고 하면, 마이티 트레이드가 사용할 비밀번호 열쇠(Bot Token)를 발급해 줍니다.',
        mockup: 'tg-newbot-token',
        actions: [
          '채팅창에 /newbot 을 입력해 보냅니다.',
          '봇의 표시 이름을 편하게 입력합니다. (예: 마이티트레이드 알림)',
          '봇의 영문 아이디를 입력합니다. 반드시 끝이 bot 으로 끝나야 합니다. (예: my_mightytrade_bot)',
          '완료 메시지에 나오는 숫자:영문 형태의 긴 문자열(Token)을 클릭해 복사합니다.',
        ],
        copySnippets: [{ label: '명령어 복사', value: '/newbot' }],
        tip: '아이디가 이미 사용 중이라고 나오면 끝에 본인만 아는 숫자를 붙여 보세요. (예: mighty_alert_2026_bot)',
        caution: '발급된 Bot Token은 열쇠와 같으므로 다른 사람에게 보여주지 마세요. 마이티 트레이드 서버에는 암호화되어 저장됩니다.',
      },
      {
        id: 'tg-3',
        shortTitle: '내 봇 시작(필수)',
        title: '3단계 · 방금 만든 내 봇에게 먼저 [시작] 누르기',
        summary: '텔레그램은 스팸 방지를 위해 사용자가 먼저 대화를 걸지 않으면 봇이 메시지를 보낼 수 없게 막아 둡니다.',
        mockup: 'tg-start-mybot',
        actions: [
          '2단계 완료 메시지 맨 위에 있는 t.me/내봇아이디 파란 링크를 클릭합니다.',
          '방금 만든 내 알림 봇의 채팅방이 열리면 아래쪽 [시작 (START)] 버튼을 꼭 누릅니다.',
          '아무 말이나 한 마디(예: 안녕) 보내두면 채팅방이 활성화됩니다.',
        ],
        caution: '이 단계를 건너뛰면 테스트 알림을 보낼 때 "chat not found" 또는 전송 실패 오류가 발생합니다!',
      },
      {
        id: 'tg-4',
        shortTitle: 'Chat ID 찾기',
        title: '4단계 · 알림 받을 내 번호(Chat ID) 확인하기',
        summary: '내 봇이 누구에게 알림을 보내야 할지 알 수 있도록, 내 텔레그램 계정의 고유 숫자 번호(Chat ID)를 확인합니다.',
        mockup: 'tg-get-chatid',
        actions: [
          '방법 A (가장 쉬움): 텔레그램 검색창에 @userinfobot 을 찾아 [시작]을 누르면 바로 "Id: 123456789" 숫자를 알려줍니다.',
          '방법 B (브라우저 확인): 3단계에서 내 봇에 "안녕"을 보낸 뒤, 브라우저 주소창에 아래 getUpdates 주소를 넣어 "chat":{"id": 숫자} 를 찾습니다.',
          '확인한 숫자만(예: 123456789) 복사해 둡니다.',
        ],
        copySnippets: [
          { label: '@userinfobot 복사', value: '@userinfobot' },
          { label: '브라우저 확인 주소 양식', value: 'https://api.telegram.org/bot<여기에_봇토큰>/getUpdates' },
        ],
        links: [{ label: '@userinfobot 바로 열기 ↗', url: 'https://t.me/userinfobot' }],
        tip: '그룹 채팅방에서 받고 싶다면 내 봇을 그룹에 초대하고, 그룹 Chat ID(보통 -100으로 시작하는 음수)를 넣으면 됩니다.',
      },
      {
        id: 'tg-5',
        shortTitle: '저장 & 테스트',
        title: '5단계 · 마이티 트레이드 설정에 붙여넣고 테스트!',
        summary: '복사해 둔 2가지 값(Bot Token, Chat ID)을 설정 화면에 붙여넣고 테스트 메시지가 오는지 확인하면 끝입니다.',
        mockup: 'tg-paste-settings',
        actions: [
          '아래 [텔레그램 입력창으로 이동] 버튼을 눌러 설정 화면으로 돌아갑니다.',
          '[텔레그램 Bot Token] 칸에 2단계에서 복사한 토큰을 붙여넣습니다.',
          '[텔레그램 Chat ID] 칸에 4단계에서 확인한 숫자를 붙여넣습니다.',
          '[알림 설정 저장]을 누른 뒤 [테스트 알림 보내기] 버튼으로 텔레그램에 알림이 도착하는지 확인합니다.',
        ],
        tip: '저장된 뒤에는 보안상 토큰이 별표(마스킹)로 가려집니다. 나중에 Chat ID나 알림 항목만 바꿀 때는 토큰 칸을 비워 두고 저장해도 기존 토큰이 그대로 유지됩니다.',
      },
    ],
  },

  discord: {
    id: 'discord',
    name: '디스코드',
    emoji: '🎮',
    badge: '가장 쉬움 · 30초 완성',
    duration: '약 30초',
    difficulty: '매우 쉬움 (4단계)',
    bestFor: '복잡한 봇 생성 없이 주소(URL) 딱 1줄만 복사하면 바로 끝납니다. PC와 폰 모두 무료 푸시 알림을 지원합니다.',
    neededFields: ['디스코드 Webhook URL'],
    steps: [
      {
        id: 'dc-1',
        shortTitle: '알림 채널 준비',
        title: '1단계 · 디스코드에서 나만의 알림 서버(또는 채널) 만들기',
        summary: '다른 사람 눈치 볼 필요 없이 나만 보는 개인 디스코드 서버를 하나 만들면 깔끔하게 매매 기록을 모아볼 수 있습니다.',
        mockup: 'dc-create-server',
        actions: [
          '디스코드 앱(또는 웹) 왼쪽 서버 목록 맨 아래의 [ + ] (서버 추가하기) 버튼을 누릅니다.',
          '[직접 만들기] → [나와 친구들을 위한 서버]를 고르고 이름을 정합니다. (예: 내 주식 알림방)',
          '기본 생성된 #일반 채널을 그대로 쓰거나, #매매-알림 텍스트 채널을 하나 추가합니다.',
        ],
        links: [{ label: '디스코드 웹 바로 열기 ↗', url: 'https://discord.com/app' }],
        tip: '이미 쓰고 있는 본인 서버가 있다면 그 안의 텍스트 채널을 바로 사용해도 됩니다.',
      },
      {
        id: 'dc-2',
        shortTitle: '채널 연동 열기',
        title: '2단계 · 채널 톱니바퀴(채널 편집) → [연동] 메뉴 들어가기',
        summary: '알림이 도착할 텍스트 채널의 설정을 열어 웹후크(Webhook) 발급 화면으로 이동합니다.',
        mockup: 'dc-channel-integrations',
        actions: [
          '알림을 받을 채널 이름(예: #매매-알림) 위에 마우스를 올리고 오른쪽 톱니바퀴 [⚙️ 채널 편집] 아이콘을 누릅니다.',
          '(모바일 앱이라면 채널 이름을 길게 누른 뒤 [채널 편집]을 선택하세요.)',
          '왼쪽 메뉴에서 [연동 (Integrations)]을 누른 뒤 [웹후크 (Webhooks)]를 선택합니다.',
        ],
        tip: '웹후크는 외부 프로그램이 이 채널에 알림 글만 안전하게 올려주는 전용 우체통 주소입니다.',
      },
      {
        id: 'dc-3',
        shortTitle: '웹후크 URL 복사',
        title: '3단계 · [새 웹후크] 만들고 URL 복사하기',
        summary: '새 웹후크를 만들고 복사 버튼만 한 번 누르면 준비가 모두 끝납니다.',
        mockup: 'dc-copy-webhook',
        actions: [
          '[새 웹후크 (New Webhook)] 버튼을 누릅니다.',
          '생성된 웹후크를 눌러 이름을 보기 좋게 바꿉니다. (예: 마이티트레이드 알림)',
          '[웹후크 URL 복사 (Copy Webhook URL)] 버튼을 눌러 https://discord.com/api/webhooks/... 주소를 복사합니다.',
        ],
        caution: '이 웹후크 URL을 아는 사람은 해당 채널에 메시지를 보낼 수 있으므로 외부에 공개하지 마세요.',
      },
      {
        id: 'dc-4',
        shortTitle: '저장 & 테스트',
        title: '4단계 · 마이티 트레이드 설정에 붙여넣고 테스트!',
        summary: '복사한 주소 1줄을 붙여넣고 저장한 뒤 테스트 알림을 보내면 바로 연결이 완료됩니다.',
        mockup: 'dc-paste-settings',
        actions: [
          '아래 [디스코드 입력창으로 이동] 버튼을 눌러 설정 화면으로 돌아갑니다.',
          '[디스코드 Webhook URL] 칸에 방금 복사한 https:// 주소를 붙여넣습니다.',
          '[알림 설정 저장]을 누른 뒤 [테스트 알림 보내기] 버튼을 클릭합니다.',
          '디스코드 채널에 "🔔 [Mighty Trade 알림 연결 테스트]" 메시지가 뜨면 성공입니다!',
        ],
        tip: '스마트폰 디스코드 앱에서 해당 채널의 알림 설정을 [모든 메시지]로 켜 두면 체결 순간마다 즉시 푸시 알림이 울립니다.',
      },
    ],
  },

  slack: {
    id: 'slack',
    name: '슬랙',
    emoji: '💼',
    badge: '업무 중 확인 편리',
    duration: '약 2분',
    difficulty: '보통 (5단계)',
    bestFor: '회사나 작업 중 슬랙을 항상 켜 두는 분께 추천합니다. 별도 채널(#주식-알림)로 조용히 모아볼 수 있습니다.',
    neededFields: ['슬랙 Webhook URL'],
    steps: [
      {
        id: 'sl-1',
        shortTitle: '새 슬랙 앱 만들기',
        title: '1단계 · Slack API 페이지에서 [Create New App] 클릭',
        summary: '슬랙 공식 앱 관리 페이지에서 마이티 트레이드 알림을 받아줄 나만의 미니 앱을 만듭니다.',
        mockup: 'sl-create-app',
        actions: [
          '아래 링크로 Slack API 앱 관리 페이지(api.slack.com/apps)를 열고 로그인합니다.',
          '오른쪽 위의 초록색 [Create New App] 버튼을 누릅니다.',
          '팝업이 뜨면 첫 번째 항목인 [From scratch (처음부터 만들기)]를 선택합니다.',
        ],
        links: [{ label: 'Slack API 앱 페이지 열기 ↗', url: 'https://api.slack.com/apps' }],
        tip: '무료 슬랙 워크스페이스에서도 개수 제한 걱정 없이 개인 알림용 웹훅 앱을 바로 만들 수 있습니다.',
      },
      {
        id: 'sl-2',
        shortTitle: '이름·워크스페이스',
        title: '2단계 · 앱 이름 입력하고 알림 받을 워크스페이스 고르기',
        summary: '알림 보낸 사람으로 표시될 이름과, 알림이 도착할 내 슬랙 워크스페이스를 지정합니다.',
        mockup: 'sl-app-name',
        actions: [
          'App Name 칸에 알아보기 쉬운 이름을 입력합니다. (예: MightyTrade 알림)',
          'Pick a workspace 드롭다운에서 알림을 받을 내 슬랙 워크스페이스를 선택합니다.',
          '오른쪽 아래 [Create App] 버튼을 누릅니다.',
        ],
        caution: '회사 공용 워크스페이스는 관리자 승인이 필요할 수 있습니다. 개인용 무료 워크스페이스를 하나 만들어 쓰시면 승인 없이 바로 됩니다.',
      },
      {
        id: 'sl-3',
        shortTitle: 'Webhook 켜기',
        title: '3단계 · [Incoming Webhooks] 메뉴에서 스위치 켜기',
        summary: '외부에서 슬랙 채널로 메시지를 보낼 수 있도록 Incoming Webhooks 기능을 켭니다.',
        mockup: 'sl-enable-webhooks',
        actions: [
          '앱 설정 화면 왼쪽 메뉴(Features)에서 [Incoming Webhooks]를 클릭합니다.',
          '오른쪽 위 [Activate Incoming Webhooks] 옆의 스위치를 [Off → On]으로 켭니다.',
        ],
        tip: '스위치를 켜면 화면 아래쪽에 "Webhook URLs for Your Workspace" 영역이 새로 나타납니다.',
      },
      {
        id: 'sl-4',
        shortTitle: '채널 선택 & URL 복사',
        title: '4단계 · 알림 받을 채널 선택하고 Webhook URL 복사',
        summary: '어느 채널(#주식-알림 등)에 알림을 띄울지 연결하고 생성된 https://hooks.slack.com 주소를 복사합니다.',
        mockup: 'sl-add-channel-copy',
        actions: [
          '화면 맨 아래 [Add New Webhook to Workspace] 버튼을 누릅니다.',
          '알림을 받을 채널(예: #주식-알림 또는 나와의 DM)을 고르고 [허용 (Allow)]을 누릅니다.',
          '다시 돌아온 화면의 Webhook URL 표에서 https://hooks.slack.com/services/... 옆 [Copy] 버튼을 클릭합니다.',
        ],
        caution: '반드시 https://hooks.slack.com/services/ 로 시작하는 전체 주소를 복사해야 합니다.',
      },
      {
        id: 'sl-5',
        shortTitle: '저장 & 테스트',
        title: '5단계 · 마이티 트레이드 설정에 붙여넣고 테스트!',
        summary: '복사한 슬랙 웹훅 주소를 설정에 붙여넣고 테스트 메시지를 보내 정상 연결을 확인합니다.',
        mockup: 'sl-paste-settings',
        actions: [
          '아래 [슬랙 입력창으로 이동] 버튼을 눌러 설정 화면으로 돌아갑니다.',
          '[슬랙 Webhook URL] 칸에 복사한 https://hooks.slack.com/... 주소를 붙여넣습니다.',
          '[알림 설정 저장]을 누른 뒤 [테스트 알림 보내기]를 클릭합니다.',
          '지정한 슬랙 채널에 테스트 알림이 도착하면 모든 설정이 끝납니다!',
        ],
        tip: '텔레그램·디스코드·슬랙은 동시에 여러 개를 등록할 수도 있습니다. 등록한 모든 채널로 실시간 알림이 함께 전송됩니다.',
      },
    ],
  },
}

export const NOTIFY_CHANNEL_LIST: NotifyChannelGuide[] = [
  NOTIFY_CHANNEL_GUIDES.telegram,
  NOTIFY_CHANNEL_GUIDES.discord,
  NOTIFY_CHANNEL_GUIDES.slack,
]

export type NotifyFieldAnchor = 'tg-token' | 'tg-chat-id' | 'discord-url' | 'slack-url'

export function guideTargetForField(anchor: NotifyFieldAnchor): { channel: NotifyChannelId; stepIndex: number } {
  switch (anchor) {
    case 'tg-token':
      return { channel: 'telegram', stepIndex: 1 } // 2단계: /newbot 토큰 발급
    case 'tg-chat-id':
      return { channel: 'telegram', stepIndex: 3 } // 4단계: Chat ID 확인
    case 'discord-url':
      return { channel: 'discord', stepIndex: 1 } // 2단계: 채널 연동 → 웹후크
    case 'slack-url':
      return { channel: 'slack', stepIndex: 2 } // 3단계: Incoming Webhooks 켜기
  }
}
