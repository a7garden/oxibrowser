# OxiBrowser 남은 작업 — 2026-08-10 기준

> **작성:** 2026-08-10 · **분기:** `main`
> **상위 문서:** `docs/superpowers/specs/2026-08-09-post-phase5-remaining-roadmap.md`
> **목적:** Phase 1–9 완료 시점(2026-08-10)의 **실제 잔여 작업만** 정리한다. 다음 세션이 이
> 파일을 먼저 읽고, 완료된 작업을 반복하거나 닫힌 리스크를 다시 열지 않도록 한다.
>
> **하드 제약(상위 로드맵 §5 계승):** pure Rust 유지 — Chromium/V8 도입 금지,
> `boa_engine`/`html5ever`/Blitz 스택/`wreq`+`btls` 유지. 픽셀 퍼펙트 Chrome 패리티·안티봇
> 챌린지 풀이·DevTools 프론트엔드는 **비목표**.

---

## 1. 현재 위치 (한 줄)

**Phase 1–9 + Phase 8(iframe 분리 컨텍스트) + 폰트 + iframe srcdoc/about:blank 완료 (2026-08-10,
v0.19.0).** 검증 게이트 전부 통과: fmt, clippy `-D warnings`, workspace 테스트 전부 통과,
`--features browser` 빌드, 인수 하네스 **8/8 PASS** (부재했던 React-SPA baseline 증거 확보).

| Phase | 내용 | 상태 |
|---|---|---|
| 1–5 + follow-ups | navigate 스크립트, 이벤트 루프, 비동기 fetch, Web API, CDP 완전성 | ✅ |
| 6 | 네트워크 정확성 (CORS, 쿠키 만료/PSL/CHIPS, proxy, auth, Referer, streaming) | ✅ |
| 7 | 렌더/상호작용 (hit-testing, `printToPDF`) + @font-face 웹폰트 | ✅ (2026-08-10; 외부 stylesheet 적용만 별도 갭) |
| 8 | iframe 분리 JS 컨텍스트 + cross-frame evaluate | ✅ (2026-08-10) |
| 9 | 멀티탭, downloads, geolocation/timezone, interception(네비+JS-fetch), tracing | ✅ (2026-08-10) |

---

## 2. 남은 구현 작업 (전부)

### 2.1 [완료] Phase 8 — iframe 분리 JS 컨텍스트 + cross-frame evaluate
- **상태:** 완료 (2026-08-10). 각 iframe마다 독립된 `boa::Context` + `RenderDocument` 생성,
  스크립트 실행, `Runtime.evaluate`의 `contextId` 라우팅, `Page.getFrameTree` 자식 보고,
  `executionContextCreated` per-frame 발생. 자세한 설계는
  `docs/superpowers/specs/2026-08-10-phase8-iframe-contexts.md` 참조.

### 2.2 [완료] Phase 7 — 폰트 로딩 (`@font-face`)
- **상태:** 완료 (2026-08-10, v0.19.0). inline `<style>` `@font-face` 웹폰트 로딩 구현 —
  URL 추출 → fetch → `Collection::register_fonts` → `config.font_ctx` (공개 API, fork 불필요).
- **정정 (이전 기재 오류):** `2026-08-10` 이전 버전이 "`fontdb` 정적 `FONT_DB`가 `pub(crate)` +
  `svg` 게이트 → 포크 필요"로 지목했으나, 그 `FONT_DB`는 `usvg` **SVG 렌더링 경로**이지
  **텍스트 레이아웃이 아니다**. 텍스트 폰트는 `DocumentConfig.font_ctx`(공개) +
  `Collection::register_fonts` 경로이며, spike + e2e로 증명됨. fork/vendoring 불필요.
- **남음 (관련 갭):** 외부 `<link rel=stylesheet>` CSS 미적용(`data:` base URL panic) — 별도 갭.

### 2.3 [선택] JS-fetch 인터셉션 e2e 검증
- **상태:** 구현·배선·유닛 테스트 완료 (`e031352`):
  - core: `FETCH_PATTERNS` 정적 + `set_fetch_patterns`, `maybe_intercept` (공유
    `PausedRequestRegistry` + `CoreEvent::RequestPaused` + oneshot 대기),
    `handle_fetch_requests`가 `event_tx: Arc<RwLock<Option<Sender>>>` 수신.
  - cdp: `Fetch.enable` → core 패턴 미러링, `core_event.rs`가 RequestPaused →
    `Fetch.requestPaused` 변환 (`send_fetch_event[_with_session]`), continue/fail/fulfill은
    기존 navigate-path 핸들러 재사용.
  - 유닛 테스트: `session::tests::test_maybe_intercept_js_fetch_interception`
    (fulfill + 빈 패턴 fast-path).
- **상태:** **완료 (2026-08-10, v0.19.0).** CDP round-trip e2e가
  `acceptance/fetch-intercept.ts`로 검증됨 (4/4 PASS): 페이지 `fetch()` → `Fetch.requestPaused`
  → `Fetch.fulfillRequest` → promise가 fulfilled body로 resolve. 검증 과정에서 **버그 발견·수정**:
  `Fetch.fulfillRequest`가 `body`를 항상 base64 디코드하지 않아 base64 문자열이 그대로 전달됨 —
  spec상 body는 항상 base64 (플래그 없음). 디코드를 무조건 수행하도록 수정.
- **앵커:** `crates/oxibrowser-cdp/src/domains/fetch.rs` (fulfillRequest 디코딩 수정),
  `acceptance/fetch-intercept.ts` (e2e).

---

## 3. 닫힌 리스크 / 의존성 한계 (재개 금지)

- **실제 소스 레벨 스택 프레임** — `boa` 0.20이 `JsNativeError`에 위치 정보를,
  `Error.stack`에 실제 프레임을 주지 않음. best-effort `.stack` 전달 + 에러 `className`까지
  달성 (CHANGELOG 참조). 해결은 boa 업그레이드/포크 필요 — 범위 밖.
- **canvas/WebGL 실제 래스터화** — 의존성/범위 한계로 out-of-scope (canvas 2D shim만 존재).
- **인수 테스트 (JS SPA)** — 상위 로드맵 §6의 궁극 인수 테스트(이동 → 로그인 폼 → submit →
  대시보드 대기 → 스크린샷)가 `oxibrowser serve`에서 **통과 (2026-08-10, 8/8 PASS)**.
  `acceptance/` 하네스 + `baseline.png`/`result.json`으로 명시적 증거 확보. (가치 제안: JS
  SPA를 CDP로 end-to-end 구동.)
- **비목표:** 픽셀 퍼펙트 Chrome 렌더링 패리티, V8/네이티브 JS엔진, 안티봇 챌린지 풀이,
  DevTools 프론트엔드.

---

## 4. 검증 게이트 (매 커밋)

```bash
cargo build --features browser --bin oxibrowser   # 반드시 이 형태(아래 gotcha)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

> **빌드 gotcha:** `oxibrowser` 바이너리는 `browser` 피처 필요. `cargo build -p oxibrowser`는
> `[[bin]] required-features` 때문에 재링크하지 않고 "Finished"만 보고함 → stale 바이너리.

> **인수 프로브 gotcha (2026-08-10 관측):** `run.sh`의 서버 대기 루프(최대 10초)가 바이너리
> 콜드 스타트보다 짧아 프로브가 WS 연결도 못 열고 조용히 exit 0할 수 있음. 대기 루프를
> 200 × 0.2s로 늘리고 curl 준비 확인 후 프로브를 실행할 것 (재현 명령:
> `bun /tmp/oxi-probe/probe.ts <cdp_port> <mock_port>`, mock은 `bun /tmp/oxi-probe/mock.ts`).

---

## 5. 다음 세션 시작점

v0.19.0 (2026-08-10) 기준. W1(인수 하네스 + JS-fetch e2e), W2-core(@font-face), W3a(srcdoc/
about:blank) 완료·검증. 남은 항목:

1. **[최우선] Phase 8 컨텍스트 동시성 경화** — 중첩 iframe의 실행 컨텍스트 생성이 JS 스레드를
   **deadlock**시킴 (`inject_child_frames` → `set_frame_document` for grandchild → 이후
   `Runtime.evaluate` hang). 프레임 트리 중첩 자체는 동작 (getFrameTree가 level2 보고)하나
   컨텍스트 생성이 교착. 원인은 per-context 아키텍처(ACTIVE_CONTEXT_ID + deferred fetch/ws
   buffer)의 중첩 생성 시 취약점. 이걸 해결해야 W3b(중첩), W3c(window.parent/top), W3d(동적
   iframe) 모두 재개 가능.
2. **외부 `<link rel=stylesheet>` 적용** — `data:` base URL로 Blitz가 panic. W2-pre.
3. **알려진 갭** (인수 하네스로 발견): `window.addEventListener` 부재, JS `fetch()` 상대 URL
   미resolve("invalid URL"), `hashchange` 미발화. 상대 URL fetch는 실사이트 호환에 영향 큰 후보.
4. 인수 하네스는 `bash acceptance/run.sh`로 재실행 가능 (회귀 게이트).

끝.

끝.
