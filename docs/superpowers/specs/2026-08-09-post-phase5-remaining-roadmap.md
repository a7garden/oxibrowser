# Post-Phase-5 Remaining Roadmap — OxiBrowser → Headless-Chrome Parity

> **작성:** 2026-08-09 · **분기:** `main`
> **상위 로드맵:** `docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md`
> **목적:** Phase 4+5 follow-ups(`2026-08-09-phase5-followups-remaining-work.md`)까지
> 마감한 시점에서, **앞으로 남은 phase** 와 그 안의 구체적 작업을 정리한다. 다음 세션이
> 완료된 작업을 반복하거나 닫힌 리스크를 다시 열지 않도록 한다.
>
> **하드 제약(상위 로드맵 §5에서 계승):** pure Rust 유지 — Chromium/V8 도입 금지,
> `boa_engine`/`html5ever`/Blitz 스택/`wreq`+`btls` 유지. 픽셀 퍼펙트 Chrome 패리티·
> 안티봇 챌린지 풀이·DevTools 프론트엔드는 **비목표**.

---

## 1. 현재 위치 (한 줄)

**Phase 1–5 + 4/5 follow-ups 완료. Phase 7 일부 완료. 남은 큰 덩어리 = Phase 6(네트워크
정확성), 그 다음 Phase 7 잔여 → Phase 8 → Phase 9.**

핵심 피드백 루프(parse → `<script>` 실행 → 이벤트 루프 → 비동기 fetch → 라이브
RenderDocument → Blitz 렌더 → screenshot/CDP)는 이미 닫혀 있고 Playwright/Puppeteer
구동이 가능한 영역. 아래 표는 전체 9개 phase 기준 상태다.

| Phase | 내용 | 상태 |
|---|---|---|
| 1 | navigate 시 `<script>` 실행(키스톤) | ✅ |
| 2 | 실제 이벤트 루프(timer/microtask 독립 틱, network-idle, live `wait_for`) | ✅ |
| 3 | 비동기(논블로킹) fetch/XHR, 동시 요청 | ✅ |
| 4 | 누락 Web API(matchMedia, WebSocket, FormData/Blob, canvas2D, AbortController, Shadow DOM/lifecycle, matches/closest) | ✅ |
| 5 | CDP 완전성(Emulation, Log, flat-protocol sessionId, exceptionThrown/consoleAPICalled, DOM.*, dialog) | ✅ |
| 4+5 follow-ups | 섀도 스크린샷 래스터화, 콘솔 타입화 RemoteObject, 드레이너 종료, 섀도 DOM slot API·closed-mode·innerHTML·declarative | ✅ (이번 분기) |
| 6 | 네트워크 정확성(CORS/preflight, 쿠키 만료/PSL/CHIPS, proxy, auth, Referer, streaming) | ✅ (2026-08-09) |
| 7 | 렌더/상호작용 정확도(hit-testing, printToPDF) | 🟡 (폰트 §3.2 Blitz private API로 차단) |
| 8 | iframe/다중 프레임 | ❌ 미착수 |
| 9 | Playwright 롱테일(geolocation/timezone, downloads, 멀티탭, tracing) | 🟡 부분(geolocation/timezone + downloads 완료) |

---

## 2. 권장 진행 순서

1. **(선행) 궁극 인수 테스트 baseline 확정** — Phase 6 착수 전, 상위 로드맵 §6의 인수
   테스트(React SPA: 이동 → 로그인 폼 작성 → submit → 대시보드 대기 → 스크린샷이
   `oxibrowser serve`에서 통과)를 한 번 돌려 현재 어디까지 통과하는지 기록한다. Phase 1–5
   가 닫혔으므로 대부분 통과할 것으로 예상되나, 명시적 증거가 아직 없다. [INFERENCE]
2. **Phase 6 — 네트워크 정확성** (§4). real-site 자동화에서 인증/쿠키/크로스오리진이
   걸리는 빈도가 가장 높아 실사용 관점 임팩트가 가장 크다.
3. **Phase 7 잔여** (§3) — hit-testing을 추정치에서 실제 Taffy 레이아웃 박스 기반으로
   전환(클릭 신뢰성 직결). 폰트·PDF는 충실도 영역으로 후순위 가능.
4. **Phase 8 — iframe/다중 프레임** (§5).
5. **Phase 9 — Playwright 롱테일** (§6).

---

## 3. Phase 7 잔여 (렌더/상호작용 정확도)

이미 완료된 부분: JS DOM ↔ Blitz `RenderDocument` 단일 라이브 트리 통합, 커스텀 엘리먼트
lifecycle 콜백, 레이아웃 기반 geometry(`getBoxModel`/`getContentQuads`/`getNodeForLocation`
via `LayoutEngine`), Shadow DOM composition + 섀도 스크린샷(compose-then-feed).

### 3.1 레이아웃 기반 hit-testing (우선)
- **문제:** `document.elementFromPoint(x,y)`가 여전히 **추정치**다. DOM 순서 + 태그별 높이
  휴리스틱으로 Y 위치를 근사한다.
- **앵커:** `crates/oxibrowser-core/src/js/runtime.rs:6455`(elementFromPoint 네이티브),
  `runtime.rs:10239`(`estimate_element_height`). 사용처: `crates/oxibrowser-core/src/js/
  input.rs`(마우스/드래그 디스패치), `crates/oxibrowser-cdp/src/domains/input.rs`.
- **방향:** Blitz `BaseDocument`가 이미 Taffy 레이아웃 박스(`node.final_layout`)를 가지고
  있으므로, `elementFromPoint`를 "해당 좌표를 포함하는 가장 깊은 페인트된 노드" 탐색으로
  교체. `RenderDocument::node_layout_rect`(`crates/oxibrowser-render/src/document.rs`)와
  `LayoutEngine::compute_rect`를 재사용.
- **영향:** 클릭/입력 hit-test 신뢰성 직결 → Playwright `click()` 정확도.

### 3.2 폰트 로딩 — `fontdb` / `@font-face`
- **문제:** 웹폰트가 로드되지 않아 텍스트 폭/레이아웃이 부정확할 수 있다.
- **방향:** Blitz에 `fontdb` 통합 + `@font-face` 규칙 파싱 → 폰트 파일 fetch/매핑.
- **비고:** 충실도 영역. 자동화(correctness) 블로커는 아님.

### 3.3 `printToPDF`
- **문제:** 현재 빈 PDF를 반환. `printpdf` 의존성 미추가.
- **앵커:** `crates/oxibrowser-cdp/src/domains/page.rs:404`(`print_to_pdf`).
- **비고:** 충실도 영역. 후순위.

---

## 4. Phase 6 — 네트워크 정확성 (다음 우선순위)

> 이 phase는 spec→plan→TDD 의 단위 분리가 아직 없다. 착수 시 전용 spec
> (`docs/superpowers/specs/<date>-phase6-network-correctness.md`)를 먼저 작성할 것.

상위 로드맵 §3 Phase 6 항목. 현재 네트워크 계층은 전송(HTTP/1.1+H2+TLS+gzip/br)은 완성,
정책 계층이 비어 있다.

| 작업 | 현재 상태 | 앵커 / 메모 |
|---|---|---|
| **CORS + preflight** | ❌ 없음 | `Origin` 송출, `Access-Control-*` 응답 헤더 해석, 사전`OPTIONS` 프리플라이트. `client.rs` |
| **쿠키 만료 / Max-Age** | ❌ | `network/cookie.rs` — 현재 jar 저장만. 만료/갱신 미구현 |
| **Public Suffix List** | ❌ | 도메인 매칭/`domain` 쿠키 스코프에 PSL 필요 |
| **CHIPS 분할 쿠키 + `__Host-`/`__Secure-`** | ❌ | 파티션 키 + prefix 검증 |
| **proxy(HTTP/SOCKS)** | ❌ | `wreq`/`btls` 설정 |
| **auth(basic/digest)** | ❌ | `Authorization` 헤더 + 401 챌린지 |
| **자동 `Referer`** | ❌ | Origin/경로 기반 Referrer 정책 |
| **스트리밍 본문** | ❌ | 현재 본문 전체 버퍼링 |

앵커 디렉토리: `crates/oxibrowser-core/src/network/`(`client.rs`, `cookie.rs`, `ip_filter.rs`,
`intercept.rs`, `resource.rs`, `ws.rs`, `robots.rs`).

**검증 바(상위 로드맵 §6):** 각 작업은 main에서 실패하는 테스트 → 통과로 마감. CORS/쿠키는
`wiremock` 기반 통합 테스트(이미 워크스페이스에 있음)로 real round-trip 검증.

---

## 5. Phase 8 — iframe / 다중 프레임

- **✅ 자식 프레임 population** (2026-08-09) — navigate가 `<iframe src>`를 fetch해
  자식 `Frame`으로 추가 (`Session::populate_iframes`). `Frame` 구조는 자식을 지원했으나
  채우지 않았음.
- **남음:** 분리된 컨텍스트에서 프레임 스크립트 실행, 프레임 경계를 가로지르는
  hit-test/evaluate (큰 규모). 앵커: `crates/oxibrowser-core/src/frame.rs`,
  `session.rs:634` (`populate_iframes`).

---

## 6. Phase 9 — Playwright 롱테일

이미 완료: `alert`/`confirm`/`prompt` dialog; **geolocation/timezone 에뮬레이션**
(2026-08-09, `navigator.geolocation` + `Emulation.setGeolocationOverride`/
`setTimezoneOverride`); **파일 다운로드** (2026-08-09, `Content-Disposition:
attachment` → 저장 + `Page.downloadWillBegin`/`downloadProgress`).

남은 항목:
- **멀티탭 (큰 규모)** — `Target.createTarget`가 stub(가짜 targetId). 진짜 구현에 필요:
  (1) `CdpSession`이 `targetId/sessionId → Arc<RwLock<Session>>` 맵 보유 (Browser의
  `sessions: Vec` 재사용 가능); (2) `create_target`가 Browser로 새 Session 생성 →
  targetId 등록 + `Target.targetCreated`/`attachedToTarget`(새 sessionId) 이벤트;
  (3) `dispatch_command`가 들어오는 메시지의 `sessionId`로 대상 Session 해석 (현재는
  항상 단일 `self.session` 사용 — `session.rs:161`); (4) 자식 타겟 이벤트에 자식
  sessionId 부착. Phase 5 flat-protocol sessionId 멀티플렉싱은 응답/이벤트에만 있고
  수신 라우팅엔 없음. 앵커: `crates/oxibrowser-cdp/src/domains/target.rs:120`,
  `crates/oxibrowser-cdp/src/session.rs:160`(dispatch ctx), 
  `crates/oxibrowser-core/src/browser.rs:48`(sessions Vec).
- **✅ request interception** (2026-08-09) — navigate 경로에서 `emit_request_paused` +
  oneshot 대기로 실배선 완료. continue/fail/fulfill 지원 (Playwright route()).
  JS-fetch 경로 인터셉션은 향후 과제.
- **tracing** — 큰 규모 (미착수).
- **✅ viewport device-metrics 실적용** (2026-08-09) — `setDeviceMetricsOverride`가
  `VIEWPORT_OVERRIDE` 정적을 통해 레이아웃 뷰포트에 실반영.

---

## 7. 비목표 / 닫힌 리스크 (재개 금지)

- **비목표(상위 로드맵 §5):** 픽셀 퍼펙트 Chrome 렌더링 패리티, V8/네이티브 JS엔진 도입,
  안티봇 챌린지 풀이, DevTools 프론트엔드.
- **닫힌 리스크**(`2026-08-09-phase5-followups-remaining-work.md` §4): dialog JS-스레드
  데드락(✅ 동시 CDP 디스패치 + `spawn_blocking`), `request.id` ↔ CDP `requestId`
  문자열 변환(✅ `oxi-{id}`), CoreEvent no-sink(✅ `Option` no-op).
- **의존성 한계로 차단된 항목:** 예외의 **실제 소스 레벨 스택 프레임** — `boa` 0.20이
  `JsNativeError`에 위치 정보를, `Error.stack`에 실제 프레임을 제공하지 않음. best-effort
  `.stack` 전달 + 에러 `className`까지만 달성(CHANGELOG 참조). 해결하려면 boa 업그레이드/
  포크 필요(범위 밖). canvas/WebGL 실제 래스터화도 동일하게 의존성/범위 한계로 out-of-scope.

---

## 8. 검증 게이트 (매 커밋)

```bash
cargo build --features browser --bin oxibrowser   # 항상 이 형태(아래 gotcha)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

> **빌드 gotcha:** `oxibrowser` 바이너리는 `browser` 피처가 필요. `cargo build -p
> oxibrowser`는 `[[bin]] required-features` 때문에 바이너리를 재링크하지 않고 "Finished"
> 만 보고함 → stale 바이너리. 반드시 `--features browser --bin oxibrowser`로 빌드.

---

끝.
