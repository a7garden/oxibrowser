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

**Phase 1–9 전부 완료 (2026-08-10).** Phase 9의 마지막 잔여였던 JS-fetch 인터셉션까지
마감(커밋 `e031352`). 남은 작업: Phase 8 iframe 분리 컨텍스트(대규모, §5), 폰트(차단, §3.2),
JS-fetch e2e 검증(선택). **최신 잔여 작업 목록은 `2026-08-10-remaining-work.md`를 먼저
읽을 것.**

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
| 7 | 렌더/상호작용 정확도(hit-testing, printToPDF) | ✅ (2026-08-09; 폰트 §3.2만 차단) |
| 8 | iframe/다중 프레임 | 🟡 (자식 프레임 population 완료; 분리 컨텍스트/cross-frame 잔여) |
| 9 | Playwright 롱테일(멀티탭, downloads, geolocation/timezone, interception, tracing) | ✅ (2026-08-10, JS-fetch 인터셉션 포함) |

---

## 2. 권장 진행 순서 (2026-08-10 기준 — Phase 6·7·9 완료, 진행 순서는 `2026-08-10-remaining-work.md` §5 참조)

1. **(남은 유일 기능 작업) Phase 8 — iframe 분리 컨텍스트/cross-frame** (§5). 대규모 —
   착수 시 전용 spec 먼저.
2. **(선택) 궁극 인수 테스트 baseline 확정** — 상위 로드맵 §6의 인수 테스트(React SPA:
   이동 → 로그인 폼 작성 → submit → 대시보드 대기 → 스크린샷)를 `oxibrowser serve`에서
   한 번 돌려 현재 어디까지 통과하는지 기록한다. 명시적 증거가 아직 없다. [INFERENCE]
3. **(선택) JS-fetch 인터셉션 CDP round-trip e2e** — 유닛 테스트는 완료(`e031352`),
   라이브 서버 e2e만 남음.
4. **폰트 (§3.2)** — 차단 유지(Blitz 포크 결정 전까지 재개 금지).

---

## 3. Phase 7 잔여 (렌더/상호작용 정확도)

이미 완료된 부분: JS DOM ↔ Blitz `RenderDocument` 단일 라이브 트리 통합, 커스텀 엘리먼트
lifecycle 콜백, 레이아웃 기반 geometry(`getBoxModel`/`getContentQuads`/`getNodeForLocation`
via `LayoutEngine`), Shadow DOM composition + 섀도 스크린샷(compose-then-feed).

### 3.1 레이아웃 기반 hit-testing — ✅ 완료 (2026-08-09)
- `hit_test_element`(`crates/oxibrowser-core/src/js/runtime.rs`)로 렌더 문서 레이아웃 기반
  히트 테스트 전환, `SetDocument`가 렌더 문서 기준 DomSnapshot 구성(id 일관성). 커밋 `96949fe`.

### 3.2 폰트 로딩 — `fontdb` / `@font-face`
- **문제:** 웹폰트가 로드되지 않아 텍스트 폭/레이아웃이 부정확할 수 있다.
- **방향:** Blitz에 `fontdb` 통합 + `@font-face` 규칙 파싱 → 폰트 파일 fetch/매핑.
- **비고:** 충실도 영역. 자동화(correctness) 블로커는 아님.

### 3.3 `printToPDF` — ✅ 완료 (2026-08-09)
- `printpdf`/`png` 피처로 실 PDF 반환. 커밋 `bf29f0b`.

---

## 4. Phase 6 — 네트워크 정확성 — ✅ 완료 (2026-08-09)

> 전용 spec: `2026-08-09-phase6-network-correctness.md` (커밋 `def1854`→`af6222f`).

상위 로드맵 §3 Phase 6 항목. 현재 네트워크 계층은 전송(HTTP/1.1+H2+TLS+gzip/br)은 완성,
정책 계층이 비어 있다.

| 작업 | 현재 상태 | 앵커 / 메모 |
|---|---|---|
| **CORS + preflight** | ✅ | `network/cors.rs` — Origin 송출, `Access-Control-*` 해석, `OPTIONS` 프리플라이트 |
| **쿠키 만료 / Max-Age** | ✅ | `network/cookie.rs` — `Expires`(`httpdate`), `Max-Age<=0` 삭제, lazy purge |
| **Public Suffix List** | ✅ | `psl` crate — `Domain=` 공개 접미사 거부, `registrable_domain` eTLD+1 |
| **CHIPS 분할 쿠키 + `__Host-`/`__Secure-`** | ✅ | prefix 검증 + `Partitioned`/`partition_key` |
| **proxy(HTTP/SOCKS)** | ✅ | `BrowserConfig.proxy` + `serve --proxy` |
| **auth(basic/digest)** | ✅ | `network/auth.rs` — 401 챌린지 재시도 (`request_with_auth`) |
| **자동 `Referer`** | ✅ | strict-origin-when-cross-origin, `CURRENT_ORIGIN` + `FetchRequestMsg.origin` |
| **스트리밍 본문** | ✅ | wreq `bytes_stream` + `stream` 피처 |

커밋 범위: `def1854`→`af6222f` (2026-08-09). 전용 spec: `2026-08-09-phase6-network-correctness.md`.

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

남은 항목: **없음** — Phase 9 전부 완료 (2026-08-10, JS-fetch 인터셉션 포함).

완료 내역:
- **✅ 멀티탭** — `Target.createTarget`가 실 Browser 세션 생성 (`child_targets`
  맵 등록 + `targetCreated`/`attachedToTarget`). `dispatch_command`가 들어오는
  명령의 `sessionId`로 대상 세션 해석. 자식 탭 navigate/evaluate/DOM 동작.
  **자식 세션 JS-발생 이벤트**(console/exception/fetch/WS)도 `emit_core_event_with_session`
  drainer로 자식 sessionId와 함께 발신됨 (2026-08-09).
- **✅ request interception** — navigate 경로에서 `emit_request_paused` + oneshot
  대기로 실배선. continue/fail/fulfill (Playwright route()). **JS-fetch 경로
  인터셉션도 완료** (2026-08-10, 커밋 `e031352` — `Fetch.enable` 패턴 → core
  `FETCH_PATTERNS` 미러링, `maybe_intercept` 페이즈, `CoreEvent::RequestPaused` →
  `Fetch.requestPaused` 변환; 유닛 테스트 `test_maybe_intercept_js_fetch_interception`).
  CDP round-trip e2e는 `2026-08-10-remaining-work.md` §2.3 선택 항목.
- **✅ tracing** — Tracing 도메인 (start/end/getCategories + dataCollected/
  tracingComplete). 풀 timeline/network tracer는 범위 밖.
- **✅ viewport device-metrics 실적용** — `VIEWPORT_OVERRIDE` 정적이 레이아웃에 반영.

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
