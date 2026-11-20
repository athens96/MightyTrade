# MightyTrade

국내/미국 증시 시세·차트 조회 및 (추후) 주문·자동주문을 위한 트레이드 웹.

- **server/** — Flask API (Python 3.10+)
- **client/** — React + Vite + TypeScript
- **deploy/** — Oracle Cloud(VM) 배포용 nginx / systemd 설정

## 구조

```
server/app/
├── __init__.py          create_app(): 설정 로드, 스토리지 초기화, blueprint 등록
├── config.py            환경변수 기반 설정 (.env)
├── errors.py            ApiError → JSON 에러, NotImplementedError → 501
├── crypto.py            DB 저장 비밀값(OTP secret, 증권사 키) 암호화 (Fernet)
├── transport_crypto.py  클라→서버 비밀값 전송 암호화 (RSA-OAEP-256 + AES-256-GCM)
├── storage/             저장소 추상화
│   ├── base.py            Store 인터페이스 (get/put/delete/find)
│   └── json_store.py      파일 DB 구현 (컬렉션당 JSON 파일, 파일락 + 원자적 쓰기)
├── auth/                구글 로그인 + OTP (2단계 세션)
│   ├── google.py          Google ID 토큰 검증, 허용 이메일 체크
│   ├── otp.py             TOTP 등록/검증, 실패 횟수 잠금
│   ├── tokens.py          JWT httpOnly 쿠키 (pending / session)
│   ├── decorators.py      @login_required, @pending_required
│   └── routes.py          /api/auth/*
├── integrations/toss/   토스증권 API 클라이언트 (토큰 공유 캐시, 재발급, 429 재시도, 에러 매핑)
├── account/             계좌: 보유종목·손익·매수가능금액, 조건주문(OCO 익절/손절, 기본 드라이런 + OTP)
├── realtime/            실시간 중계: 사용자별 토스 웹소켓 1개 → 브라우저 SSE (호가 5회/초로 합침)
├── users/               계정별 데이터 (관심종목, 설정, 증권사 API 키)
├── market/              시세/차트
│   ├── models.py          Market(KR/US), Interval, Candle, Quote
│   ├── registry.py        시장별 provider 선택 (MARKET_PROVIDER_KR/US)
│   ├── aggregate.py       1m→5m/15m/1h, 1d→1w 봉 집계
│   ├── providers/         mock / toss(시세·차트·검색) / investing(TODO)
│   ├── extras.py          토스 전용: 지수, 장 운영 상태, 투자경고/VI, 투자자 수급, 호가 스냅샷
│   └── routes.py          /api/market/candles, /quote(s), /search, /overview, /warnings, /investor-flow, /orderbook
├── quant/               퀀트 연구 (백테스트)
│   ├── indicators.py      지표 계산 (클라이언트 TS와 동일 결과, shared/fixtures로 검증)
│   ├── engine.py          백테스트 엔진: 종가 판단 → 다음 봉 체결, 시장가/스톱, 수수료·세금·슬리피지
│   ├── metrics.py         CAGR, MDD, 샤프/소르티노/칼마, 승률, 손익비 …
│   ├── optimize.py        파라미터 격자 탐색 (최소 매매 수, 주변 평균으로 안정성 평가)
│   ├── walkforward.py     워크포워드 검증 (롤링/누적, 검증 구간만 이어붙인 수익곡선, WFE)
│   ├── montecarlo.py      매매 부트스트랩/셔플 → 수익률·MDD 분포, 손실 확률
│   ├── strategies/        전략 (백테스트·실전 공용 인터페이스): 이평 크로스, 변동성 돌파, RSI(2), 볼린저, 터틀
│   └── routes.py          /api/quant/strategies, /defaults, /backtest, /optimize, /walkforward, /montecarlo
├── trading/             주문: 미리보기→OTP 잠금 해제(30분)→전송, 호가단위·상하한가·잔고 검증, 일일/건당 한도, 감사 기록
└── autotrade/           자동매매 봇: 모의(기본)/실거래, 스케줄러(정규장 시작 1분 뒤), 중복 주문 방지, 체결 반영, 전체 중지
    ├── models.py          Order, OrderRequest, Position
    ├── brokers/           Broker 인터페이스, TossBroker(TODO)
    └── automation/        Strategy / Signal 인터페이스

client/src/
├── api/                 fetch 래퍼 + auth/market/trading API
├── auth/AuthContext.tsx 로그인 상태 (anonymous → pending → authenticated)
├── lib/secureEnvelope.ts 비밀값 브라우저 측 암호화 (WebCrypto)
├── indicators/          기술적 지표 계산(math/calc) + 목록·기본값·설명(registry), vitest 테스트
├── components/chart/    TradingChart(멀티 패널·범례), IndicatorPanel(지표 선택), BandFill(밴드/구름 채우기)
├── components/          Layout, ProtectedRoute
└── pages/               Login, Otp, Dashboard(차트), Orders, AutoTrade, Settings(토스 키)
```

## 인증 흐름

1. 클라이언트 Google 로그인 → `POST /api/auth/google {credential}`
2. 서버가 ID 토큰 검증 → 사용자 upsert → **pending 쿠키** 발급 (OTP 관련 API만 접근 가능, 10분)
3. 최초: `POST /api/auth/otp/setup` → QR 등록 / 이후: 바로 코드 입력
4. `POST /api/auth/otp/verify {code}` → **session 쿠키** 발급 → 모든 `/api/*` 접근 가능

쿠키는 httpOnly + SameSite=Strict. 클라이언트와 API를 같은 origin으로 서비스(개발: Vite proxy, 운영: nginx)하므로 CORS가 필요 없음.

## 로컬 실행

`.env` 준비 후 `./scripts/dev.sh` 한 번으로 서버+클라이언트 동시 실행 (Ctrl+C로 둘 다 종료). 개별 실행은 아래 참고.

사전 준비: Google Cloud Console에서 OAuth **웹 클라이언트 ID** 생성, 승인된 JavaScript 원본에 `http://localhost:5173` 추가.

```bash
# server
cd server
python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt
cp .env.example .env        # GOOGLE_CLIENT_ID 입력
.venv/bin/python wsgi.py    # http://127.0.0.1:5001
.venv/bin/pytest

# client
cd client
npm install
cp .env.example .env        # VITE_GOOGLE_CLIENT_ID 입력
npm run dev                 # http://localhost:5173
npm test                    # 지표 계산 테스트
```

시세는 기본적으로 `mock` provider(결정적 랜덤 데이터)라 API 연동 전에도 차트가 표시됨.

## 토스증권 API 키 (계정별)

각 사용자가 **설정 → 토스증권 API 연결**에서 자기 키를 등록. 서버 전역 키는 없음.

| 구간 | 보호 방식 |
|---|---|
| 브라우저 → 서버 | HTTPS + 앱 레벨 암호화: 서버 RSA-3072 공개키로 일회용 AES-256-GCM 키를 감싸 전송. AAD=사용자 ID(다른 계정에 재사용 불가), `iat` 5분 유효 |
| 등록 승인 | OTP 재확인 (코드 1회용) + 토스에 실제 토큰 발급·계좌 조회로 검증 후 저장 |
| 저장 | `ENCRYPTION_KEY`(Fernet)로 암호화. DB에는 암호문 + 마스킹된 client_id만 존재 |
| 응답 | 비밀값은 어떤 API로도 반환되지 않음 (상태·마스킹 값만) |
| 사용 | 서버에서만 복호화해 토스 호출. 시세 캐시도 사용자별로 분리(토스 약관: 재배포 금지) |

`ENCRYPTION_KEY`는 DB와 분리 보관(서버 환경변수). DB만 유출돼도 키는 복호화 불가. 이 키를 잃으면 저장된 키·OTP를 복구할 수 없음.
WebCrypto는 보안 컨텍스트(HTTPS 또는 localhost)에서만 동작하므로 운영은 반드시 HTTPS.

## 토스증권 API 주의사항

- 실시간 시세는 서버가 중계(토스 웹소켓 → SSE). 운영은 gunicorn **gthread 단일 워커**로 실행(`deploy/mighty-trade-api.service`), nginx는 `/api/realtime/` 버퍼링 끔(`deploy/nginx.conf`).
- 조건주문(OCO)은 실제 주문: 기본은 미리보기(드라이런), 실제 전송은 확인 체크 + 1회용 OTP 필요. 국내는 KRX 정규장에서만 발동.

- 허용 IP 등록 필수 (WTS > 설정 > Open API > 허용 IP 관리). 운영 서버는 고정(예약) 공인 IP 사용.
- 각 사용자는 자기 토스 WTS의 허용 IP에 **이 서비스 서버의 IP**를 등록해야 함.
- 클라이언트 키당 유효 토큰은 1개: 새로 발급하면 이전 토큰이 즉시 무효. 서버는 토큰을 저장소에 암호화 캐시해 프로세스 간 공유함.
  **같은 키로 로컬과 운영 서버를 동시에 돌리지 말 것** (서로 토큰을 무효화함 — 자동 재발급되지만 불필요한 호출 발생).
- 데이터는 본인 투자 목적으로만 사용 가능 (제3자 재배포 금지).
- 봉은 1m/1d만 제공 → 5m/15m/1h는 1분봉 최대 2000개로 집계(1h는 약 30여 개 봉), 1w는 일봉으로 집계.

## 퀀트 랩 (백테스트)

- 새 전략 추가: `server/app/quant/strategies/`에 `Strategy`를 상속한 클래스(`prepare` / `on_bar`)를 만들고 `strategies/__init__.py`의 `STRATEGIES`에 등록. 화면에는 자동으로 나타남.
- 체결 규칙: 바 i 종가에 판단한 주문은 바 i+1에서만 체결(미래 정보 차단). 롱 전용, 한 번에 한 포지션, 정수 주식.
- 지표를 바꾸면 `cd server && .venv/bin/python -m app.quant.fixtures`로 fixture 재생성 → 서버·클라이언트 테스트가 두 구현의 일치를 검증.

## 거래 · 자동매매

**수동 주문** (대시보드 오른쪽 패널 → 주문, 계좌 → 주문 내역)
- 주문 확인 = 서버 미리보기(전송 안 함): 호가 단위 맞춤, 상·하한가, 매수 가능 금액/매도 가능 수량, 장 운영 상태 경고.
- 전송은 **거래 잠금 해제**(OTP, 기본 30분) 상태에서만. 취소는 잠금 없이 가능, 정정은 잠금 필요.
- 한도: 하루 주문 수, 주문 1건 금액(`.env`의 `TRADING_*`). 모든 주문은 `orders` 컬렉션에 기록.

**자동매매** (메뉴 → 자동매매, 또는 퀀트 랩 백테스트 → "이 설정으로 봇 만들기")
- 봇 = 종목 + 퀀트 랩 전략 + 파라미터 + 운용 자금. 만들면 **모의 모드·꺼짐**.
- 매일 정규장 시작 1분 뒤 전날 종가까지의 일봉으로 판단(백테스트와 동일 규칙). 장 마감 1시간 전 이후 시작이면 그날은 건너뜀.
  - 시장가 신호 → 모의: 오늘 시가 체결 / 실거래: 시장가 주문
  - 돌파·손절 신호 → 모의: 다음 실행 때 그날 고가·저가로 정산 / 실거래: 토스 조건주문(SINGLE, 당일 유효), 이미 돌파했으면 시장가
- 실거래 전환은 OTP + 확인 필요, 전환 시 꺼짐. 실거래 주문도 계정 한도를 따르고 매수는 체결 여유 0.5%를 둠.
- 중복 방지: 실행일을 주문 전에 저장(장애 시 하루를 건너뛸 뿐 두 번 주문하지 않음) + 봇·날짜 기반 고정 clientOrderId.
- **전체 중지**: 모든 봇 정지 + 봇이 낸 미체결 주문 취소. 재개는 OTP.
- 스케줄러는 서빙 프로세스 안에서 돌고 파일 잠금으로 서버당 하나만 실행(개발 서버 리로더 감시 프로세스에서는 미실행).
- 토스는 모의투자 환경이 없음: 실거래 경로는 가짜 응답 테스트로 검증됨. 처음에는 작은 운용 자금으로 시작할 것.

## 저장소 교체 (파일 DB → Supabase 등)

비즈니스 코드는 `get_store()`만 사용. `storage/base.py`의 `Store`를 구현한 클래스를 추가하고 `STORAGE_BACKEND`로 선택하면 됨. 컬렉션: `users`, `watchlists`, `settings`, `orders`, `strategies`.

## 배포 (Oracle Cloud)

1. VM에 `/opt/mighty-trade`로 배포, `server/.env` 작성 (`FLASK_ENV=production`, `SECRET_KEY`, `ENCRYPTION_KEY`, `COOKIE_SECURE=true`)
2. `client`에서 `npm run build` → `client/dist`
3. `deploy/mighty-trade-api.service` → systemd, `deploy/nginx.conf` → nginx
4. VCN Security List / OS 방화벽에서 80/443 개방, certbot으로 HTTPS

## TODO

- [x] 토스 시세·차트·종목검색 (`TossProvider`)
- [x] 토스 계좌 조회 (보유종목·매수가능금액), 조건주문 OCO 익절/손절
- [x] 일반 주문 (지정가/시장가, 정정·취소, 실시간 체결 알림)
- [x] 자동매매 봇 (모의/실거래)
- [ ] 미국 종목 영문명 검색 (`/stocks/all`에 englishName 없음 → `/stocks` 배치 조회 필요)
- [ ] investing.com 데이터 수집 방식 결정 (공식 API 없음 — 이용약관 확인 필요)
- [ ] 주문 API (검증 → 리스크 체크 → 브로커 → 저장)
- [x] 퀀트 랩 1단계: 단일 종목 일봉 백테스트 + 전략 5개
- [x] 퀀트 2단계: 파라미터 최적화 히트맵, 워크포워드, 몬테카를로
- [ ] 퀀트 3단계: ETF 자산배분/듀얼모멘텀 (포트폴리오, 리밸런싱)
- [ ] 저장소 백엔드 결정 (Supabase 유력)
