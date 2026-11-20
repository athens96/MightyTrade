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
├── users/               계정별 데이터 (관심종목, 설정, 증권사 API 키)
├── market/              시세/차트
│   ├── models.py          Market(KR/US), Interval, Candle, Quote
│   ├── registry.py        시장별 provider 선택 (MARKET_PROVIDER_KR/US)
│   ├── aggregate.py       1m→5m/15m/1h, 1d→1w 봉 집계
│   ├── providers/         mock / toss(시세·차트·검색) / investing(TODO)
│   └── routes.py          /api/market/candles, /quote, /search
└── trading/             주문·자동주문 (스텁, 501 응답)
    ├── models.py          Order, OrderRequest, Position
    ├── brokers/           Broker 인터페이스, TossBroker(TODO)
    └── automation/        Strategy / Signal 인터페이스

client/src/
├── api/                 fetch 래퍼 + auth/market/trading API
├── auth/AuthContext.tsx 로그인 상태 (anonymous → pending → authenticated)
├── lib/secureEnvelope.ts 비밀값 브라우저 측 암호화 (WebCrypto)
├── components/          Layout, ProtectedRoute, CandleChart(lightweight-charts)
└── pages/               Login, Otp, Dashboard(차트), Orders, AutoTrade, Settings(토스 키)
```

## 인증 흐름

1. 클라이언트 Google 로그인 → `POST /api/auth/google {credential}`
2. 서버가 ID 토큰 검증 → 사용자 upsert → **pending 쿠키** 발급 (OTP 관련 API만 접근 가능, 10분)
3. 최초: `POST /api/auth/otp/setup` → QR 등록 / 이후: 바로 코드 입력
4. `POST /api/auth/otp/verify {code}` → **session 쿠키** 발급 → 모든 `/api/*` 접근 가능

쿠키는 httpOnly + SameSite=Strict. 클라이언트와 API를 같은 origin으로 서비스(개발: Vite proxy, 운영: nginx)하므로 CORS가 필요 없음.

## 로컬 실행

사전 준비: Google Cloud Console에서 OAuth **웹 클라이언트 ID** 생성, 승인된 JavaScript 원본에 `http://localhost:5173` 추가.

```bash
# server
cd server
python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt
cp .env.example .env        # GOOGLE_CLIENT_ID 입력
.venv/bin/python wsgi.py    # http://127.0.0.1:5000
.venv/bin/pytest

# client
cd client
npm install
cp .env.example .env        # VITE_GOOGLE_CLIENT_ID 입력
npm run dev                 # http://localhost:5173
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

- 허용 IP 등록 필수 (WTS > 설정 > Open API > 허용 IP 관리). 운영 서버는 고정(예약) 공인 IP 사용.
- 각 사용자는 자기 토스 WTS의 허용 IP에 **이 서비스 서버의 IP**를 등록해야 함.
- 클라이언트 키당 유효 토큰은 1개: 새로 발급하면 이전 토큰이 즉시 무효. 서버는 토큰을 저장소에 암호화 캐시해 프로세스 간 공유함.
  **같은 키로 로컬과 운영 서버를 동시에 돌리지 말 것** (서로 토큰을 무효화함 — 자동 재발급되지만 불필요한 호출 발생).
- 데이터는 본인 투자 목적으로만 사용 가능 (제3자 재배포 금지).
- 봉은 1m/1d만 제공 → 5m/15m/1h는 1분봉 최대 2000개로 집계(1h는 약 30여 개 봉), 1w는 일봉으로 집계.

## 저장소 교체 (파일 DB → Supabase 등)

비즈니스 코드는 `get_store()`만 사용. `storage/base.py`의 `Store`를 구현한 클래스를 추가하고 `STORAGE_BACKEND`로 선택하면 됨. 컬렉션: `users`, `watchlists`, `settings`, `orders`, `strategies`.

## 배포 (Oracle Cloud)

1. VM에 `/opt/mighty-trade`로 배포, `server/.env` 작성 (`FLASK_ENV=production`, `SECRET_KEY`, `ENCRYPTION_KEY`, `COOKIE_SECURE=true`)
2. `client`에서 `npm run build` → `client/dist`
3. `deploy/mighty-trade-api.service` → systemd, `deploy/nginx.conf` → nginx
4. VCN Security List / OS 방화벽에서 80/443 개방, certbot으로 HTTPS

## TODO

- [x] 토스 시세·차트·종목검색 (`TossProvider`)
- [ ] 토스 계좌 조회 / 주문 (`TossBroker`)
- [ ] 미국 종목 영문명 검색 (`/stocks/all`에 englishName 없음 → `/stocks` 배치 조회 필요)
- [ ] investing.com 데이터 수집 방식 결정 (공식 API 없음 — 이용약관 확인 필요)
- [ ] 주문 API (검증 → 리스크 체크 → 브로커 → 저장)
- [ ] 자동주문 스케줄러 (gunicorn과 별도 프로세스)
- [ ] 저장소 백엔드 결정 (Supabase 유력)
