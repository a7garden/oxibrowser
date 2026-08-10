# Phase 8 — Iframe 분리 JS 실행 컨텍스트 + cross-frame evaluate

> **작성:** 2026-08-10 · **분기:** `main`
> **상위 문서:** `docs/superpowers/specs/2026-08-10-remaining-work.md` §2.1
> **목적:** iframe별 분리된 JS 실행 컨텍스트를 구현하여, CDP 클라이언트(Puppeteer/
> Playwright)가 특정 iframe을 타겟으로 `Runtime.evaluate`를 실행할 수 있게 한다.
> 자동화 가치: iframe 기반 페이지에서 동적 콘텐츠 스크래핑.

---

## 1. 현황 (구축된 것)

| 기능 | 상태 |
|---|---|
| iframe HTML fetch + 정적 `Frame` 스냅샷 추가 | ✅ `populate_iframes` (`session.rs:844`) |
| `Frame` API (`id`, `url`, `html`, `document`, `children`, `extract_scripts`) | ✅ `frame.rs:63` |
| 단일 `boa::Context` + 단일 `RenderDocument` (JS 스레드) | ✅ `js_thread_loop` (`runtime.rs:1555`) |
| `Runtime.executionContextCreated` 하드코딩 `id:1` | ✅ `runtime.rs:38` (CDP) |
| `Page.getFrameTree` → `childFrames: []` | ✅ `page.rs:344` (CDP, 자식 미보고) |

**현황 한 줄:** iframe은 fetch + 정적 파싱까지만 됨. iframe 스크립트 미실행, live
`RenderDocument` 없음, CDP 컨텍스트 분리 없음.

---

## 2. 목표 아키텍처

### 2.1 핵심 결정: 다중 Context (per-frame boa Context)

각 프레임(루트 + iframe 자식)마다 독립된 `boa_engine::Context` + `RenderDocument`를
JS 스레드에 보유한다. 단일 Context + document-swap 대안을 배제한 이유:

- boa GC는 Context마다 독립 힙 → 다중 Context가 레지스트리 분리를 자연스럽게 강제
- 글로벌 스코프 오염 방지 (iframe `var`가 부모로 누출되지 않음)
- 브라우저 semantics 일치 (프레임 = 별도 realm)
- document-swap 대안도 node_id 충돌 문제가 동일하게 발생 → 결국 namespacing 필요

### 2.2 ACTIVE_CONTEXT_ID 패턴

```rust
thread_local! {
    static ACTIVE_CONTEXT_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}
```

JS 스레드 루프가 어떤 컨텍스트의 `eval`/pump에 진입하기 전에 `ACTIVE_CONTEXT_ID`를
설정한다. 모든 thread-local 레지스트리 함수(`registry_add`, `push_event`, fetch/ws
생성 closure)는 이 값을 읽어 엔트리를 태깅한다. 단일 Context 시절과의 하위 호환:
기존 호출자는 암묵적으로 ACTIVE_CONTEXT_ID(=1)를 사용.

### 2.3 FrameContext (JS 스레드 보유)

```rust
struct FrameContext {
    ctx: Context,
    job_queue: Rc<TokioJobQueue>,
    render_doc_cell: Rc<RefCell<Option<RenderDocument>>>,
    dom_snapshot_arc: Arc<RwLock<Option<DomSnapshot>>>,
    url: String,
}

// js_thread_loop 내부:
let mut frames: HashMap<u32 /*context_id*/, FrameContext> = HashMap::new();
// 시작 시 context_id=1 (메인 프레임) 생성
```

**공유 리소스 (프레임별 아님):** `console_output`, `mutations`, `fetch_tx_arc`,
`cookie_jar_arc`, `local_storage_tx_arc`. fetch는 fetch_id(스레드 전역 유일)로 라우팅,
쿠키는 세션 단위 공유 — 다중 컨텍스트에서도 단일 채널 유지 OK.

### 2.4 ID 체계

| 식별자 | 범위 | 할당 |
|---|---|---|
| `context_id` (u32) | JS 실행 컨텍스트 | 메인=1, 자식=2,3,... (세션 내 증가, 재사용 안 함) |
| `frame_id` (FrameId, "frame-N") | DOM 프레임 | `FrameId::next()` (기존, 불변) |
| 매핑 | `Session` 보유 | `HashMap<FrameId, u32>` + `next_context_id` 카운터 |

메인 context_id=1 유지 → 기존 CDP 클라이언트 호환성 보존.

---

## 3. 구현 단계

### Phase A: 코어 — 다중 Context JS 스레드 인프라

**A1. ACTIVE_CONTEXT_ID + 레지스트리 namespacing**
- `ACTIVE_CONTEXT_ID` thread-local 추가
- `LISTENER_REGISTRY` 키를 `(context_id, node_id, event_type)`로 변경
  - `registry_add/get/remove`가 ACTIVE_CONTEXT_ID를 읽어 태깅
- `PendingFetch`에 `context_id: u32` 필드 추가 (fetch 생성 closure가 ACTIVE_CONTEXT_ID로 태깅)
- `WsState`에 `context_id` 추가 (WS 생성 closure가 태깅)

**A2. cross-context fetch/WS settle**
- `DEFERRED_RESPONSES: RefCell<Vec<FetchResponseMsg>>` thread-local 추가
- `drain_pending_fetch_responses(active_ctx_id)`:
  1. deferred 버퍼 + 채널에서 모든 응답 수집
  2. 각 응답의 `PendingFetch.context_id == active_ctx_id`면 settle, 아니면 deferred로 재버퍼
- WS 이벤트에 동일 패턴 (`DEFERRED_WS_EVENTS`)
- **근거:** 단일 JS 스레드가 한 번에 하나의 컨텍스트만 처리 → 타 컨텍스트 응답은
  해당 컨텍스트가 다음 pump할 때 settle. 지연 발생 가능하나 v1 수용 (스크래핑은
  주로 메인 프레임이 활성).

**A3. JsCommand에 context_id 추가**
- `Eval { context_id: u32, expression, ... }` — `evaluate_with_timeout_and_await`가
  `context_id` 파라미터 추가 (기본값 1)
- `SetDocument { context_id: u32, ... }` — 기존 (메인 프레임용, context_id=1)
- **새 명령** `SetFrameDocument { context_id, html, base_url, viewport, scripts, ... }`
  — 자식 프레임 Context 생성 + RenderDocument 빌드 + 스크립트 실행
- `Capture`/`Query`/`GetDocumentSnapshot` — context_id 추가 (기본값 1; 자식 프레임
  스냅샷은 CDP DOM 도메인 확장 시 사용, v1은 메인만)

**A4. js_thread_loop 리팩터링**
- 단일 `ctx`/`render_doc_cell`/`job_queue` → `HashMap<u32, FrameContext>`
- 시작 시 context_id=1 생성 (기존 `create_context` 호출, page_url="")
- 각 명령 처리 전 `ACTIVE_CONTEXT_ID.set(context_id)` 설정
- `SetFrameDocument` 수신 시: 새 `create_context` 호출 → `frames.insert(context_id, fc)`
  → RenderDocument 빌드 → `run_navigation_scripts` → Done 응답
- Eval/SetDocument: `frames.get_mut(&context_id)`로 라우팅
- 타임아웃 시 해당 프레임 Context만 재생성 (전체가 아닌)

**검증 (Phase A):**
- 기존 602 테스트 전부 통과 (context_id 기본값 1 → 동작 변경 없음)
- `test_eval_in_main_context` 신규: 메인 컨텍스트에서 eval → 기존과 동일 결과

### Phase B: 네비게이션 — 자식 프레임 컨텍스트 빌드

**B1. Session 필드 추가**
```rust
/// frame_id → context_id 매핑 (CDP 라우팅용)
frame_contexts: parking_lot::RwLock<HashMap<String /*"frame-N"*/, u32>>,
/// 다음 자식 context_id 할당 번호 (2부터 시작, 메인=1)
next_context_id: AtomicU32,  // init 2
```

**B2. inject_child_frames()**
- `inject_dom_snapshot()` 끝에 호출 (메인 프레임 주입 후)
- 각 자식 `Frame`에 대해:
  1. `next_context_id.fetch_add(1)`로 context_id 할당
  2. 자식 HTML (`frame.html()`) + URL (`frame.url()`) + scripts (`frame.extract_scripts()`)
  3. 자식의 외부 `<script src>` fetch (메인과 동일 패턴, `inject_dom_snapshot` 참조)
  4. `js_runtime.set_frame_document(context_id, html, url, viewport, scripts).await`
  5. `frame_contexts`에 `frame.id().to_string() → context_id` 저장
- 실패 시 warn 로그 + 스킵 (깨진 iframe이 네비게이션을 중단시키지 않음 — 기존 패턴)

**B3. navigate() 수정**
- 기존 `populate_iframes` 호출은 유지 (자식 Frame 생성)
- `inject_dom_snapshot()` 후 `inject_child_frames()` 호출
- 네비게이션 시 기존 자식 컨텍스트 정리: `frame_contexts` 클리어, JS 스레드에
  `ClearChildContexts` 명령 전송 (context_id=1 제외한 모든 FrameContext drop)

**B4. JsRuntime 공개 API**
```rust
/// 자식 프레임의 Context + RenderDocument를 빌드하고 스크립트를 실행한다.
pub async fn set_frame_document(
    &mut self, context_id: u32, html: &str, base_url: &str,
    viewport: (u32, u32), scripts: Vec<ScriptSource>,
) -> Result<()>

/// 메인(context_id=1) 외의 모든 FrameContext를 drop한다 (네비게이션 정리).
pub fn clear_child_contexts(&mut self)
```

**B5. evaluate API 확장**
```rust
/// 지정된 context_id에서 eval. 기존 evaluate는 context_id=1로 위임.
pub async fn evaluate_in_context(
    &mut self, expression: &str, context_id: u32, await_promise: bool,
) -> Result<JsEvalResult>
```

**검증 (Phase B):**
- `test_iframe_creates_child_context`: iframe 1개 페이지 네비게이션 →
  `session.frame_contexts`에 자식 매핑 존재 →
  `evaluate_in_context(expr, child_context_id)`가 iframe DOM의 텍스트 반환
- `test_child_frame_scripts_execute`: iframe에 `<script>` 포함 →
  자식 컨텍스트에서 eval 시 스크립트 결과 관찰

### Phase C: CDP — 프레임 트리 + 컨텍스트 라우팅

**C1. Page.getFrameTree — 자식 프레임 보고**
- `page.rs:344` 재귀 확장: `root_frame().children()`를 순회하여 `childFrames` 배열 생성
- 각 프레임: `{ id, url, securityOrigin, mimeType, parentId }`

**C2. Page.navigate — 자식 프레임 이벤트**
- 네비게이션 완료 후, 기존 메인 프레임 `frameNavigated`에 이어:
  - 각 자식 프레임에 대해 `Page.frameNavigated` 이벤트 발생
  - 각 자식 컨텍스트에 대해 `Runtime.executionContextCreated` 발생
- 이전 자식 컨텍스트에 대해 `Runtime.executionContextDestroyed` 발생 (선택, v1)

**C3. Runtime.enable — 모든 기존 컨텍스트 보고**
- `runtime.rs:38` 확장: 메인(id=1) + Session의 `frame_contexts`에 있는 모든 자식
  컨텍스트에 대해 `executionContextCreated` 발생
- 각 컨텍스트의 `auxData: { frameId: "frame-N", isDefault: true, type: "default" }`

**C4. Runtime.evaluate — contextId 라우팅**
- `runtime.rs:65` `evaluate()`: params에서 `contextId` 추출 (기본값 1)
- `Session::evaluate_js_in_context(expr, context_id, await_promise)` 호출
- 존재하지 않는 context_id → 에러 응답

**C5. Runtime.callFunctionOn — executionContextId 라우팅**
- params에서 `executionContextId` 추출 → 동일 라우팅

**C6. core_event.rs — executionContextId 태깅**
- `Runtime.consoleAPICalled`/`exceptionThrown`의 `executionContextId`를
  CoreEvent에 포함된 context_id 사용 (또는 ACTIVE_CONTEXT_ID)

**검증 (Phase C):**
- CDP e2e: `Page.getFrameTree`가 자식 프레임 반환
- CDP e2e: `Runtime.evaluate` with child `contextId` → iframe DOM 쿼리 성공
- 기존 raw-CDP acceptance probe 10/10 회귀 없음

---

## 4. 리스크 & 완화

| 리스크 | 영향 | 완화 |
|---|---|---|
| thread-local 레지스트리 키 변경 → 광범위 수정 | 높음 | ACTIVE_CONTEXT_ID로 기존 호출자 암묵 호환; 단계적 검증 |
| cross-context fetch settle 지연 | 중간 | deferred 버퍼; v1은 메인 프레임 중심 스크래핑 가정 |
| Context 생성 비용 (iframe 많은 페이지) | 낮음 | 대부분 0-3 iframe; 타임아웃 시 해당 프레임만 재생성 |
| CoreEvent에 context_id 추가 → Clone/변경 | 낮음 | CoreEvent는 이미 Clone; 필드 추가만 |
| Puppeteer executionContextDestroyed 기대 | 중간 | v1은 파괴 이벤트 생략 또는 best-effort; 스크래핑에는 영향 없음 |

---

## 5. 검증 게이트 (매 커밋)

```bash
cargo build --features browser --bin oxibrowser   # browser 피처 필수 (stale 바이너리 주의)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                              # 기존 602 + 신규 회귀 없음
```

**인수 프로브 (iframe 페이지):** 자식 프레임이 있는 mock 페이지 →
`Page.getFrameTree` 자식 보고 → `Runtime.evaluate` with child contextId →
iframe DOM 텍스트 반환. 기존 10/10 프로브도 회귀 없음 확인.

---

## 6. 비목표 (v1)

- `window.parent` / `window.top` 크로스 프레임 접근 (postMessage 등)
- iframe `srcdoc` / `about:blank` 컨텍스트 (http/https iframe만)
- 동적 iframe 추가/제거 추적 (`MutationObserver` 기반 컨텍스트 생성/파괴)
- same-origin 정책 강제 (모든 프레임 컨텍스트가 동일 세션 쿠키/스토리지 공유)
- 중첩 iframe (iframe 안의 iframe) — v1은 1단계 자식만; 구조적으로 재귀 가능하나
  테스트 범위 밖

끝.
