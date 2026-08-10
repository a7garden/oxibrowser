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

**Phase 1–9 전부 완료 (2026-08-10).** Phase 9의 마지막 잔여였던 **JS-fetch 인터셉션**
(`Fetch.enable` → 페이지 `fetch()`/XHR 페이즈 → `Fetch.requestPaused` → continue/fail/fulfill)
구현·배선·유닛 테스트 완료(커밋 `e031352`). 검증 게이트 전부 통과: fmt, clippy `-D warnings`,
workspace 602 tests / 0 failed, `--features browser` 바이너리 빌드, raw-CDP 인수 프로브
**10/10 PASS**.

| Phase | 내용 | 상태 |
|---|---|---|
| 1–5 + follow-ups | navigate 스크립트, 이벤트 루프, 비동기 fetch, Web API, CDP 완전성 | ✅ |
| 6 | 네트워크 정확성 (CORS, 쿠키 만료/PSL/CHIPS, proxy, auth, Referer, streaming) | ✅ |
| 7 | 렌더/상호작용 (hit-testing `hit_test_element`, `printToPDF` 실 PDF) | 🟡 (폰트만 차단, §3) |
| 8 | iframe population 완료 | 🟡 (분리 컨텍스트/cross-frame 잔여, §2) |
| 9 | 멀티탭, downloads, geolocation/timezone, interception(네비+JS-fetch), tracing | ✅ (2026-08-10) |

---

## 2. 남은 구현 작업 (전부)

### 2.1 [대규모] Phase 8 — iframe 분리 JS 컨텍스트 + cross-frame evaluate
- **상태:** 미착수. 자식 프레임 **population만** 완료 — `Session::populate_iframes`가
  `<iframe src>`를 fetch해 자식 `Frame`으로 추가 (커밋 `87e75bc`).
- **남음:** 프레임별 분리된 JS 실행 컨텍스트 (현재 런타임이 페이지당 `Context` 하나 소유),
  프레임 경계를 가로지르는 hit-test/evaluate (`Runtime.evaluate`의 `contextId` 선택).
- **앵커:** `crates/oxibrowser-core/src/frame.rs`, `session.rs::populate_iframes`.
- **비고:** 깊은 재구성(per-frame JS context) 필요. 자동화(correctness) 가치는 iframe 페이지
  대상 스크래핑에서 높음.

### 2.2 [차단] Phase 7 — 폰트 로딩 (`@font-face`)
- **상태:** 차단. Blitz 0.3.0-beta.1의 `fontdb` 정적 `FONT_DB`가 `pub(crate)`이고 `svg`
  피처 뒤에 게이트됨 (`blitz-dom/src/util.rs`) → 폰트 추가는 외부 crate 포크 필요.
- **영향:** 웹폰트 미로드 → 텍스트 폭/레이아웃 부정확 가능. **충실도(fidelity) 영역 —
  자동화 블로커 아님.**
- **권장:** 상위 로드맵에 명시된 대로 우선순위 최하위. 재개하려면 Blitz 포크 또는
  `fontdb` 로드 경로의 upstream 변경 대기.

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
- **남음:** 실제 CDP round-trip e2e (페이지 스크립트의 `fetch()`가 `Fetch.requestPaused`로
  페이즈 → `Fetch.fulfillRequest` → promise가 mock body로 resolve) 미검증. 선택 항목 —
  빈 패턴일 때 fast-path no-op이라 acceptance 경로 회귀는 없음 (프로브 10/10으로 확인).
- **앵커:** `crates/oxibrowser-cdp/src/domains/fetch.rs` (enable/continue/fail/fulfill),
  `crates/oxibrowser-cdp/src/core_event.rs` (RequestPaused arm),
  `crates/oxibrowser-core/src/session.rs` (`maybe_intercept`, `url_matches_patterns`).

---

## 3. 닫힌 리스크 / 의존성 한계 (재개 금지)

- **실제 소스 레벨 스택 프레임** — `boa` 0.20이 `JsNativeError`에 위치 정보를,
  `Error.stack`에 실제 프레임을 주지 않음. best-effort `.stack` 전달 + 에러 `className`까지
  달성 (CHANGELOG 참조). 해결은 boa 업그레이드/포크 필요 — 범위 밖.
- **canvas/WebGL 실제 래스터화** — 의존성/범위 한계로 out-of-scope (canvas 2D shim만 존재).
- **인수 테스트 (React SPA)** — 상위 로드맵 §6의 궁극 인수 테스트(이동 → 로그인 폼 →
  submit → 대시보드 대기 → 스크린샷)가 `oxibrowser serve`에서 통과하는지의 **baseline
  기록이 아직 없음**. Phase 1–9가 닫혔으므로 대부분 통과할 것으로 예상되나 명시적 증거
  부재 [INFERENCE]. 다음 세션에서 1회 실행해 기록할 것.
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

1. 작업 우선순위: **2.1 (iframe 분리 컨텍스트)** 가 유일한 남은 기능 작업 — 대규모이므로
   착수 시 전용 spec 먼저 (`docs/superpowers/specs/<date>-phase8-iframe-contexts.md`).
2. 2.3 (JS-fetch e2e) 또는 3의 React SPA 인수 baseline을 먼저 닫고 싶다면 그렇게 해도 됨 —
   어느 쪽도 블로커 아님.
3. 2.2 (폰트)는 차단 유지 — 재개 금지 unless Blitz 포크 결정.

끝.
