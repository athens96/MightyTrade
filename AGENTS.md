# Mighty Trade — Agent & MightyWork Integration Rules

## 1. MightyWork 연동 정보
이 저장소(`/Users/young/MightyTrade`)는 **MightyWork**의 **마이티 트레이드 (`MT`)** 프로젝트와 연동되어 있습니다.

- **MCP 서버**: `mightywork`
- **프로젝트 키 (`project`)**: `MT`
- **프로젝트 이름**: 마이티 트레이드 (`id`: `3b9607ded5c3439ea724ad6e08d5c09c`)
- **일감 파이프라인 단계**: `backlog` → `todo` → `in_progress` → `review` → `qa` → `done`

### 일감(Task) · 위키(Wiki) 운영 규칙
1. **작업 시작 전**: `mightywork` MCP의 `list_tasks(project="MT")` 또는 `get_task`로 관련 일감(`MT-1` 등)과 요구사항·체크리스트를 확인합니다.
2. **상태 동기화**:
   - 일감을 착수할 때 `update_task(project="MT", task="MT-...", stage="in_progress")`로 옮깁니다.
   - 하위 체크리스트 완료 시 `check_items`를 갱신하고, 주요 설계 결정·검증 결과·커밋 해시는 `add_comment`로 남깁니다.
   - 구현·검증 완료 시 요청 맥락에 따라 `review` / `qa` / `done` 단계로 갱신합니다.
3. **커밋 메시지**: 관련 일감 키가 있을 때는 커밋 메시지나 본문에 `MT-번호`를 함께 표기합니다 (예: `feat(autotrade): ... (MT-1)`).
4. **위키 문서화**: 아키텍처·전략 가이드·운영 절차 등 지속 관리할 문서는 `list_wiki_pages` / `create_wiki_page` / `update_wiki_page`(`project="MT"`)로 관리하며, 문서 간 연결은 `[[페이지 제목]]`, 일감 참조는 `MT-번호` 형식을 사용합니다.

## 2. 프로젝트 공통 원칙
- **쉬운 한글 설명**: 화면 안내, 툴팁(`title` / `HoverTooltip`), 전략·차트 해설은 금융·개발 비전공자도 바로 이해할 수 있는 쉬운 일상 한국어로 작성합니다.
- **검증 필수**:
  - 백엔드 (Rust, 프로덕션 기본): `cd server-rs && cargo test`
  - 프론트엔드: `cd client && npm test && npm run build`
  - 백엔드 (Python 레거시 참조용): `cd server && .venv/bin/pytest -q`
- **버전 관리**: 루트 `VERSION`, `client/package.json`, `server-rs/Cargo.toml`을 `./scripts/bump-version.sh <patch|minor|major|X.Y.Z>`로 함께 갱신하고, `CHANGELOG.md`와 Git 태그(`vX.Y.Z`)를 유지합니다.
- **보안·안전장치**: `.env`, `deploy/.env.deploy`, `server/data/*` 등 민감 정보는 절대 노출하거나 커밋하지 않으며, 실거래 주문 관련 변경 시 OTP 잠금·일일/건당 주문 한도·중복 주문 방지(`clientOrderId`) 원칙을 유지합니다.
