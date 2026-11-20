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
│   ├── json_store.py      파일 DB 구현 (컬렉션당 JSON 파일, 파일락 + 원자적 쓰기)
│   └── sqlite_store.py    SQLite(WAL) 구현 + JSON→SQLite 마이그레이션 유틸리티
├── auth/                구글 로그인 + OTP (2단계 세션)
│   ├── google.py          Google ID 토큰 검증, 허용 이메일 체크
│   ├── otp.py             TOTP 등록/검증, 실패 횟수 잠금
│   ├── tokens.py          JWT httpOnly 쿠키 (pending / session)
│   ├── decorators.py      @login_required, @pending_required
│   └── routes.py          /api/auth/*
├── integrations/toss/   토스증권 API 클라이언트 (토큰 공유 캐시, 재발급, 429 재시도, 에러 매핑)
├── admin/               관리자: 가입 승인·거절, 계정 삭제(OTP 재확인, 데이터 전체 삭제)
├── account/             계좌: 보유종목·손익·매수가능금액, 조건주문(OCO 익절/손절, 기본 드라이런 + OTP)
├── realtime/            실시간 중계: 사용자별 토스 웹소켓 1개 → 브라우저 SSE (호가 5회/초로 합침)
├── users/               계정별 데이터 (관심종목, 설정, 증권사 API 키)
├── market/              시세/차트
│   ├── models.py          Market(KR/US), Interval, Candle, Quote
│   ├── registry.py        시장별 provider 선택 (MARKET_PROVIDER_KR/US)
│   ├── aggregate.py       1m→5m/15m/1h, 1d→1w 봉 집계
│   ├── providers/         mock / toss(시세·차트·검색 + US 영문명 배치 보강)
│   ├── extras.py          토스 전용: 지수, 장 운영 상태, 투자경고/VI, 투자자 수급, 호가 스냅샷
│   └── routes.py          /api/market/candles, /quote(s), /search, /overview, /warnings, /investor-flow, /orderbook
├── quant/               퀀트 연구 (백테스트 · 포트폴리오)
│   ├── indicators.py      지표 계산 (클라이언트 TS와 동일 결과, shared/fixtures로 검증)
│   ├── engine.py          백테스트 엔진: 종가 판단 → 다음 봉 체결, 시장가/지정가/스톱, 수수료·세금·슬리피지
│   ├── metrics.py         CAGR, MDD, 샤프/소르티노/칼마, 승률, 손익비 …
│   ├── optimize.py        파라미터 격자 탐색 (최소 매매 수, 주변 평균으로 안정성 평가)
│   ├── walkforward.py     워크포워드 검증 (롤링/누적, 검증 구간만 이어붙인 수익곡선, WFE)
│   ├── montecarlo.py      매매 부트스트랩/셔플 → 수익률·MDD 분포, 손실 확률
│   ├── portfolio.py       멀티자산 자산배분 6종(올웨더·60/40·1/N·리스크패리티·듀얼모멘텀·VAA) 리밸런싱 엔진
│   ├── worker.py          최적화·워크포워드·몬테카를로·포트폴리오용 spawn 프로세스 풀 (GIL 격리)
│   ├── strategies/        스윙 7종(이평·터틀·MACD·볼린저·RSI2·IBS·TSMOM) + 단타 4종(변동성돌파·갭회귀·ORB·VWAP)
│   └── routes.py          /api/quant/strategies, /defaults, /backtest, /optimize, /walkforward, /montecarlo, /portfolio/*
├── trading/             주문: 미리보기→OTP 잠금 해제(30분)→전송, 호가단위·상하한가·잔고 검증, 일일/건당 한도, 감사 기록
│   ├── orders.py          주문 검증·전송·조회·취소·정정, 잠금/한도
│   ├── ticks.py           KRX/US 호가 단위 반올림·포맷
│   └── routes.py          /api/trading/*
└── autotrade/           자동매매 봇: 스윙봇·단타봇(일봉/분봉)·리밸런싱 봇, 모의(기본)/실거래, 전체 중지
    ├── bots.py            단일·멀티자산 봇 설정·상태·이벤트 로그 저장
    ├── executor.py        스윙(일봉)·단타(5m/15m 분봉 + 장마감 청산) 모의/실거래 실행
    ├── rebalance.py       멀티자산 리밸런싱 봇(매도 우선 체결 → 매수) 모의/실거래 실행
    ├── scheduler.py       정규장 스케줄러 (파일 락으로 서버당 1개)
    └── routes.py          /api/autotrade/*

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

## 가입 승인 · 관리자

- 관리자 = `server/.env`의 `ADMIN_EMAILS`(쉼표 구분). DB가 아니라 환경 설정이라 데이터를 고쳐 관리자가 될 수 없음. 관리자는 자동 승인.
- 새 Google 로그인은 **승인 대기**로 등록되고, 승인 전에는 OTP 등록·모든 API가 막힘. 거절된 계정은 로그인 시 거절 안내.
- 관리자 메뉴(승인 대기 건수 배지): 승인 / 거절(승인 취소, 해당 봇 자동 정지) / 삭제.
- 삭제: 관리자 OTP + 이메일 입력 확인. 프로필·OTP, 관심종목·설정, 토스 키(캐시 토큰 포함), 봇·기록, 주문 기록을 삭제. 실거래 봇에 보유·미체결이 있으면 기본 거부. 자기 자신·다른 관리자는 삭제 불가.
- 이 기능 이전에 만들어진 계정은 승인된 것으로 간주.
- `ALLOWED_EMAILS`는 선택 사항(지정하면 그 이메일만 가입 신청 가능). 비우면 누구나 신청하고 관리자가 승인.

## 거래 · 자동매매

**수동 주문** (대시보드 오른쪽 패널 → 주문, 계좌 → 주문 내역)
- 주문 확인 = 서버 미리보기(전송 안 함): 호가 단위 맞춤, 상·하한가, 매수 가능 금액/매도 가능 수량, 장 운영 상태 경고.
- 전송은 **거래 잠금 해제**(OTP, 기본 30분) 상태에서만. 취소는 잠금 없이 가능, 정정은 잠금 필요.
- 한도: 하루 주문 수, 주문 1건 금액(`.env`의 `TRADING_*`). 모든 주문은 `orders` 컬렉션에 기록.

**자동매매** (메뉴 → 자동매매, 또는 퀀트 랩 백테스트/포트폴리오 → "봇 만들기")
- **스윙봇 (7종)**: 이평 크로스(다우 이론), 터틀(돈치안+ATR 사이징), MACD 추세+ATR 추적손절, 볼린저 회귀, RSI(2) 평균회귀, IBS 내부봉 회귀, 시계열 모멘텀(TSMOM·변동성 타겟팅). 매일 정규장 시작 1분 뒤 전일 종가 기준 실행.
- **단타봇 (4종)**: 변동성 돌파(래리 윌리엄스), 오프닝 갭 회귀(지정가), 오프닝 레인지 돌파(ORB, 15분봉), VWAP 밴드 평균회귀(5분봉). 분봉 단타봇은 봉 마감마다 실행하며 **장 마감 10분 전 강제 청산**.
- **리밸런싱 봇 (6종)**: 올웨더(레이 달리오), 60/40 클래식(MPT), 동일 비중(1/N), 리스크 패리티(역변동성), 글로벌 듀얼 모멘텀(GEM), VAA(13612W 카나리아). 월/분기/연 정기 또는 비중 편차 이탈 시 **매도 우선 체결 → 매수** 순서로 자동 리밸런싱.
- 만들면 **모의 모드·꺼짐**. 실거래 전환은 OTP + 확인 필요, 전환 시 꺼짐. 실거래 주문도 계정 한도를 따르고 매수는 체결 여유 0.5%를 둠.
- 중복 방지: 실행일(분봉은 봉 시각)을 주문 전에 저장 + 봇·날짜·종목 기반 고정 clientOrderId.
- **전체 중지**: 모든 봇 정지 + 봇이 낸 미체결 주문 취소. 재개는 OTP.
- 스케줄러는 서빙 프로세스 안에서 돌고 파일 잠금으로 서버당 하나만 실행(개발 서버 리로더 감시 프로세스·스폰 워커에서는 미실행).
- 토스는 모의투자 환경이 없음: 실거래 경로는 가짜 응답 테스트로 검증됨. 처음에는 작은 운용 자금으로 시작할 것.

## 저장소 교체 (`json` → `sqlite`)

비즈니스 코드는 `get_store()`만 사용. `.env`에서 `STORAGE_BACKEND=sqlite`로 지정하면 SQLite(WAL 모드, `DATA_DIR/mighty_trade.db`)로 동작하며, 기존 JSON 데이터는 아래 명령으로 1회 이관 가능:

```bash
cd server && .venv/bin/python -m app.storage.sqlite_store ./data
```

## 배포 (Oracle Cloud)

처음 한 번 (Ubuntu VM):

1. VCN Security List와 OS 방화벽(`iptables` + `netfilter-persistent save`)에서 80/443 개방
2. `apt install nginx python3-venv certbot python3-certbot-nginx git rsync`, 시간대 `Asia/Seoul`
3. `/opt/mighty-trade`에 git clone, `server/.venv` 생성 후 `pip install -r requirements.txt`
4. `server/.env` 작성: `FLASK_ENV=production`, 새 `SECRET_KEY`·`ENCRYPTION_KEY`, `COOKIE_SECURE=true`, `DATA_DIR`, `GOOGLE_CLIENT_ID`, `ADMIN_EMAILS` (권한 600)
5. `deploy/mighty-trade-api.service` → systemd, `deploy/nginx.conf` → `/etc/nginx/sites-available/` (`server_name` 수정)
6. 도메인(예: DuckDNS)을 VM 공인 IP로 지정 → `certbot --nginx -d <도메인> --redirect` (자동 갱신 타이머 포함)
7. 외부 설정: Google OAuth 클라이언트의 승인된 JavaScript 원본에 `https://<도메인>`, 토스 WTS 허용 IP에 VM 공인 IP

업데이트: main에 푸시한 뒤 `./scripts/deploy.sh`. 클라이언트를 로컬에서 빌드해 올리고, 서버는 git pull, 의존성 설치, 재시작까지 한다. 접속 정보는 `deploy/.env.deploy`(git 제외)에 `DEPLOY_HOST`, `DEPLOY_KEY`, `DEPLOY_URL`로 둔다.

서버의 `server/.env`와 `server/data/`는 서버에만 있다. `ENCRYPTION_KEY`를 잃으면 저장된 토스 키와 OTP를 복구할 수 없으니 따로 백업한다.

## TODO

- [x] 토스 시세·차트·종목검색 (`TossProvider`)
- [x] 토스 계좌 조회 (보유종목·매수가능금액), 조건주문 OCO 익절/손절
- [x] 일반 주문 (지정가/시장가, 정정·취소, 실시간 체결 알림)
- [x] 자동매매 봇 (스윙봇·단타봇·리밸런싱 봇, 모의/실거래)
- [x] 미국 종목 영문명 검색 (`/stocks` 배치 조회로 백필)
- [x] investing.com 종목 바로가기 링크 제공 (외부 링크로 전환, 미구현 프로바이더 제거)
- [x] 주문 API (검증 → 리스크 체크 → 브로커 → 저장)
- [x] 퀀트 랩 1단계: 단일 종목 백테스트 + 스윙 7종·단타 4종
- [x] 퀀트 2단계: 파라미터 최적화 히트맵, 워크포워드, 몬테카를로
- [x] 퀀트 3단계: ETF 자산배분/듀얼모멘텀/VAA/리스크패리티/올웨더 (포트폴리오, 리밸런싱)
- [x] 저장소 백엔드: SQLite(WAL 모드) + JSON→SQLite 마이그레이션 유틸리티
