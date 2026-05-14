# Session B: CDP + Performance (P0.4 + P1.1 + P3 전체)

> **브랜치**: `feat/cdp-perf`
> **기준 커밋**: `b94dd2c` (main)
> **수정 파일**: `fetch.rs`, `page.rs`, `event.rs` (CDP crate), `Cargo.toml`, `core_bench.rs`
> **금지 파일**: `crates/oxibrowser-core/src/js/runtime.rs`, `crates/oxibrowser-core/src/session.rs`, `crates/oxibrowser-core/src/frame.rs`

---

## 컨텍스트

OxiBrowser CDP 서버는 `oxibrowser-cdp` 크레이트에 구현됨.
HTTP 엔드포인트 (`/json/version`, `/json`) + WebSocket 메시지 디스패치.

### CDP 아키텍처

```
CdpServer (HTTP listener)
  ├── /json/version → JSON 버전 정보
  ├── /json → 타겟 목록
  └── /ws → WebSocket 업그레이드
       └── CdpSession (per-connection)
            ├── 메시지 수신 루프
            ├── domains::dispatch() → 도메인 핸들러 라우팅
            └── EventSender → 이벤트 브로드캐스트

DispatchContext {
    session: Arc<RwLock<Session>>,  // oxibrowser-core Session
    events: EventSender,            // 이벤트 전송기
}

DomainResult = Result<Option<serde_json::Value>, CdpError>
```

### EventSender 구조 (`event.rs`)

```rust
pub struct EventSender {
    event_tx: tokio::sync::mpsc::UnboundedSender<CdpEvent>,
    page_enabled: AtomicBool,
    runtime_enabled: AtomicBool,
    network_enabled: AtomicBool,
    fetch_enabled: AtomicBool,
    fetch_patterns: Arc<RwLock<Vec<FetchPattern>>>,
}

impl EventSender {
    pub fn send_fetch_event(&self, method: &str, params: Value) { ... }
    pub fn set_fetch_patterns(&self, patterns: Vec<FetchPattern>) { ... }
    pub fn get_fetch_patterns(&self) -> Vec<FetchPattern> { ... }
}
```

### Fetch 도메인 현황 (`fetch.rs`, 257줄)

```rust
// 이미 구현됨:
pub fn handle(method, params, ctx) → DomainResult  // 라우터
fn enable(params, ctx) → DomainResult               // 패턴 저장
fn disable(ctx) → DomainResult                       // 패턴 삭제
fn continue_request(params, ctx) → DomainResult      // stub
fn fail_request(params, ctx) → DomainResult          // stub
fn fulfill_request(params, ctx) → DomainResult       // stub

pub fn emit_request_paused(events, url, method, request_id)  // 이벤트 발송 함수 존재
pub struct FetchPattern { url_pattern, .. }
pub fn matches_patterns(url, patterns) → bool
```

**문제**: `emit_request_paused`는 존재하지만 아무도 호출하지 않음.
`Session::navigate()`가 HTTP 요청을 보내기 전에 Fetch 패턴을 체크하지 않음.

### Page 도메인 현황 (`page.rs`)

```rust
fn capture_screenshot(params) → DomainResult {
    // 현재: 1×1 black PNG stub 반환
    // 목표: format="text" 인 경우 CSS text screenshot 반환
}
```

`page.to_text_screenshot()` 은 이미 `oxibrowser-core`에 구현됨:

```rust
// oxibrowser-core/src/page.rs
pub fn to_text_screenshot(&self) -> String {
    let snapshot = self.root_frame.to_dom_snapshot();
    crate::css::render_to_text(&snapshot)
}
```

---

## 작업 항목

### Task 1: P0.4 — Fetch Interception 연동

**목표**: `Fetch.enable()`로 설정한 패턴이 `Page.navigate()` 시 HTTP 요청 전에 체크되고,
매칭되면 `Fetch.requestPaused` 이벤트가 발송되도록 연동.

**파일**: `crates/oxibrowser-cdp/src/domains/fetch.rs`, `crates/oxibrowser-cdp/src/domains/page.rs`, `crates/oxibrowser-cdp/src/domains/mod.rs`

#### Step 1: DispatchContext에 paused_requests 추가

`mod.rs`의 `DispatchContext`에 paused request 저장소 추가:

```rust
use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::HashMap;

/// A paused Fetch request awaiting client decision.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PausedRequest {
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub resource_type: String,
    pub frame_id: String,
}

// DispatchContext에 추가:
pub struct DispatchContext {
    pub session: Arc<tokio::sync::RwLock<oxibrowser_core::Session>>,
    pub events: EventSender,
    // 추가:
    pub paused_requests: Arc<RwLock<HashMap<String, PausedRequest>>>,
    pub mock_responses: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}
```

**주의**: `DispatchContext`는 `CdpSession`이 생성합니다.
`server.rs` 또는 `session.rs` (CDP crate)에서 `DispatchContext::new()`를 호출하는 곳에서
`paused_requests`와 `mock_responses`를 초기화해야 함.

`CdpSession`이 이미 `EventSender`를 클론해서 `DispatchContext`에 전달하는 패턴을 따름:

```bash
grep -n "DispatchContext" crates/oxibrowser-cdp/src/session.rs
grep -n "DispatchContext" crates/oxibrowser-cdp/src/domains/mod.rs
```

#### Step 2: navigate()에서 Fetch 패턴 체크

`page.rs`의 `navigate()` 핸들러 수정.

**현재** (navigate 함수 내):
```rust
let result = {
    let mut session = ctx.session.write().await;
    session.navigate(&url).await
};
```

**수정 후**:
```rust
// 1. Fetch 패턴 체크
let patterns = ctx.events.get_fetch_patterns();
let matched = if !patterns.is_empty() {
    crate::domains::fetch::matches_patterns(&url, &patterns)
} else {
    false
};

if matched {
    // 2. Paused request 생성
    let request_id = format!("req-{}", uuid::Uuid::new_v4());
    let paused = PausedRequest {
        request_id: request_id.clone(),
        url: url.clone(),
        method: "GET".to_string(),
        resource_type: "Document".to_string(),
        frame_id: String::new(),
    };

    // 3. 저장
    ctx.paused_requests.write().insert(request_id.clone(), paused.clone());

    // 4. requestPaused 이벤트 발송
    crate::domains::fetch::emit_request_paused(
        &ctx.events,
        &url,
        "GET",
        &request_id,
    );

    // 5. 실제 네비게이션은 보류 → client가 continueRequest/fulfillRequest로 결정
    // 지금은 이벤트만 발송하고, navigate는 계속 진행 (mock이 있으면 사용)
    if let Some(mock) = ctx.mock_responses.read().get(&request_id) {
        // Mock response 사용
        // TODO: Session에 mock HTML 주입
    } else {
        // 정상 진행
        let result = {
            let mut session = ctx.session.write().await;
            session.navigate(&url).await
        };
        // ... 기존 로직
    }
} else {
    // 기존 navigate 로직
    let result = {
        let mut session = ctx.session.write().await;
        session.navigate(&url).await
    };
    // ...
}
```

#### Step 3: fulfillRequest 실제 동작

`fetch.rs`의 `fulfill_request()` 수정:

```rust
async fn fulfill_request(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.ok_or(CdpError { code: -32602, message: "missing params".into() })?;
    let request_id = params["requestId"].as_str().ok_or(CdpError {
        code: -32602,
        message: "missing requestId".into(),
    })?;

    // Mock response 저장
    let mock = serde_json::json!({
        "body": params["body"].as_str().unwrap_or(""),
        "status": params["responseCode"].as_u64().unwrap_or(200),
        "headers": params["responseHeaders"].clone(),
    });
    ctx.mock_responses.write().insert(request_id.to_string(), mock);

    // Paused request 제거
    ctx.paused_requests.write().remove(request_id);

    Ok(Some(serde_json::json!({})))
}
```

#### Step 4: continueRequest 실제 동작

```rust
async fn continue_request(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.ok_or(CdpError { code: -32602, message: "missing params".into() })?;
    let request_id = params["requestId"].as_str().ok_or(CdpError {
        code: -32602,
        message: "missing requestId".into(),
    })?;

    // Paused request 제거 (요청이 정상 진행됨을 의미)
    ctx.paused_requests.write().remove(request_id);

    Ok(Some(serde_json::json!({})))
}
```

#### Step 5: DispatchContext 생성 수정

`crates/oxibrowser-cdp/src/domains/mod.rs` 또는 `session.rs`에서
DispatchContext 생성 시 `paused_requests`, `mock_responses` 초기화:

```bash
# DispatchContext가 어떻게 생성되는지 확인
grep -n "DispatchContext {" crates/oxibrowser-cdp/src/ -r
```

#### 테스트 (E2E):

`crates/oxibrowser-cdp/tests/e2e.rs`에 추가:

```rust
#[tokio::test]
async fn test_fetch_request_paused_on_navigate() {
    let (mut sink, mut ws, events) = connect_cdp().await;

    // 1. Fetch.enable with pattern
    let resp = send_command(&mut sink, &mut ws, 1, "Fetch.enable",
        Some(json!({ "patterns": [{ "urlPattern": "*" }] }))).await;
    assert_eq!(resp["id"], 1);

    // 2. Page.enable
    let resp = send_command(&mut sink, &mut ws, 2, "Page.enable", None).await;
    assert_eq!(resp["id"], 2);

    // 3. Navigate (should trigger requestPaused)
    let resp = send_command(&mut sink, &mut ws, 3, "Page.navigate",
        Some(json!({ "url": "data:text/html,<h1>Test</h1>" }))).await;

    // 4. Check for requestPaused event
    // Event might arrive as separate message
    // Wait for it with timeout
    let event = wait_for_event(&mut ws, "Fetch.requestPaused", Duration::from_secs(5)).await;
    assert!(event.is_some(), "Should receive Fetch.requestPaused event");
    assert!(event.unwrap()["params"]["requestId"].is_string());

    // 5. Cleanup
    let _ = send_command(&mut sink, &mut ws, 4, "Fetch.disable", None).await;
}

#[tokio::test]
async fn test_fetch_fulfill_request() {
    let (mut sink, mut ws, _) = connect_cdp().await;

    // 1. Fetch.enable
    send_command(&mut sink, &mut ws, 1, "Fetch.enable",
        Some(json!({ "patterns": [{ "urlPattern": "*" }] }))).await;

    // 2. Navigate + wait for requestPaused
    send_command(&mut sink, &mut ws, 2, "Page.navigate",
        Some(json!({ "url": "http://example.com/" }))).await;

    let event = wait_for_event(&mut ws, "Fetch.requestPaused", Duration::from_secs(5)).await;
    let request_id = event.unwrap()["params"]["requestId"].as_str().unwrap().to_string();

    // 3. Fulfill with mock response
    let mock_html = base64::encode("<html><body>Mocked!</body></html>");
    let resp = send_command(&mut sink, &mut ws, 3, "Fetch.fulfillRequest",
        Some(json!({
            "requestId": request_id,
            "responseCode": 200,
            "body": mock_html
        }))).await;
    assert_eq!(resp["id"], 3);
}
```

**주의**: `wait_for_event` 헬퍼 함수가 필요할 수 있음.
기존 E2E 테스트에서 이벤트를 수신하는 패턴을 확인:

```bash
grep -n "event\|Event\|ws.next" crates/oxibrowser-cdp/tests/e2e.rs | head -20
```

---

### Task 2: P1.1 — captureScreenshot text mode

**목표**: `Page.captureScreenshot(format: "text")` 시 CSS text screenshot 반환

**파일**: `crates/oxibrowser-cdp/src/domains/page.rs`

#### 수정 위치: `capture_screenshot()` (line ~268)

**현재**:
```rust
fn capture_screenshot(params: Option<Value>) -> DomainResult {
    // ... 1×1 black PNG stub
}
```

**수정 후**:
```rust
fn capture_screenshot(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let format = params.as_ref()
        .and_then(|p| p.get("format"))
        .and_then(|f| f.as_str())
        .unwrap_or("png");

    match format {
        "text" => {
            // CSS text screenshot
            let session = ctx.session.try_read();
            let text = session
                .ok()
                .and_then(|s| s.page().map(|p| p.to_text_screenshot()))
                .unwrap_or_default();

            Ok(Some(json!({
                "data": base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
                "format": "text"
            })))
        }
        _ => {
            // 기존 PNG stub 유지
            // ... existing 1×1 PNG code ...
        }
    }
}
```

**주의 1**: `capture_screenshot`의 시그니처가 `fn(params) -> DomainResult` 인지
`fn(params, ctx) -> DomainResult` 인지 확인. CDP 핸들러는 보통 `ctx`를 받지 않을 수 있음.

확인:
```bash
grep -n "fn capture_screenshot" crates/oxibrowser-cdp/src/domains/page.rs
```

만약 시그니처가 `fn(params) -> DomainResult` 라면,
`page.rs`의 `handle()` 함수에서 `ctx`를 전달하도록 수정 필요:

```rust
// page.rs handle() 내부
"captureScreenshot" => capture_screenshot(params, ctx),
```

그리고 `capture_screenshot` 시그니처 변경:
```rust
fn capture_screenshot(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
```

**주의 2**: `base64` 크레이트 의존 여부 확인.
`oxibrowser-cdp/Cargo.toml`에 `base64`가 없으면 추가해야 할 수 있음.
대안: `data` 필드에 base64 인코딩 대신 raw 텍스트를 넣거나,
기존 코드에서 base64 인코딩을 어떻게 하는지 확인:

```bash
grep -n "base64\|encode" crates/oxibrowser-cdp/src/domains/page.rs | head -10
```

#### 테스트:
```rust
#[tokio::test]
async fn test_capture_screenshot_text_mode() {
    let (mut sink, mut ws, _) = connect_cdp().await;

    // Navigate first
    send_command(&mut sink, &mut ws, 1, "Page.navigate",
        Some(json!({ "url": "data:text/html,<h1>Hello</h1>" }))).await;

    // Capture as text
    let resp = send_command(&mut sink, &mut ws, 2, "Page.captureScreenshot",
        Some(json!({ "format": "text" }))).await;

    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["format"], "text");
    assert!(resp["result"]["data"].is_string());

    // Decode and verify contains "Hello"
    let encoded = resp["result"]["data"].as_str().unwrap();
    let decoded = base64::decode(encoded).unwrap();
    let text = String::from_utf8(decoded).unwrap();
    assert!(text.contains("Hello"));
}
```

---

### Task 3: P3.1 — Binary Size Optimization

**목표**: Release 빌드 크기 최적화 (~15MB → ~8MB)

**파일**: `Cargo.toml` (root)

#### Step 1: profile 섹션 추가

`Cargo.toml` 끝에 추가:

```toml
[profile.release]
opt-level = "z"       # Size optimization
lto = true            # Link-time optimization
codegen-units = 1     # Better optimization (slower compile)
strip = true          # Strip debug symbols
panic = "abort"       # Smaller unwinding tables

# JS engine은 속도 우선
[profile.release.package.boa_engine]
opt-level = 3
```

#### Step 2: 불필요 feature 비활성화 검토

```bash
# boa_engine features 확인
grep "boa_engine" Cargo.toml crates/*/Cargo.toml
```

boa_engine의 기본 features 중 불필요한 것 비활성화:
```toml
boa_engine = { version = "0.20", default-features = false, features = ["annex-b"] }
```

**주의**: `default-features = false` 시 일부 기능이 누락될 수 있으므로,
빌드 + 테스트 후 확인.

#### 검증:
```bash
cargo build --release
ls -lh target/release/oxibrowser
# 타겟: < 10MB
```

---

### Task 4: P3.2 — Startup Time Benchmark

**목표**: Browser::new() < 50ms, Session::navigate() < 100ms 측정

**파일**: `crates/oxibrowser-core/benches/core_bench.rs`

기존 벤치마크 파일에 추가. `criterion` 크레이트가 이미 설정되어 있는지 확인:

```bash
grep "criterion\|bench" crates/oxibrowser-core/Cargo.toml
```

#### 추가할 벤치마크:

```rust
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use oxibrowser_core::{Browser, BrowserConfig};

fn bench_browser_startup(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("browser_startup", |b| {
        b.to_async(&rt).iter(|| async {
            let browser = Browser::new(BrowserConfig::default()).await.unwrap();
            browser.close().await.unwrap();
        });
    });
}

fn bench_session_navigate(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let browser = rt.block_on(Browser::new(BrowserConfig::default())).unwrap();

    c.bench_function("session_navigate_data_uri", |b| {
        b.to_async(&rt).iter(|| {
            browser.new_page("data:text/html,<h1>Hello</h1>")
        });
    });

    // cleanup
    rt.block_on(browser.close()).unwrap();
}

fn bench_js_eval(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let browser = rt.block_on(Browser::new(BrowserConfig::default())).unwrap();
    let session = rt.block_on(browser.new_page("data:text/html,<p>test</p>")).unwrap();

    c.bench_function("js_eval_simple", |b| {
        b.to_async(&rt).iter(|| {
            session.write().evaluate_js("1 + 1")
        });
    });

    c.bench_function("js_eval_dom_query", |b| {
        b.to_async(&rt).iter(|| {
            session.write().evaluate_js("document.querySelector('p').textContent")
        });
    });

    rt.block_on(browser.close()).unwrap();
}

criterion_group!(benches, bench_browser_startup, bench_session_navigate, bench_js_eval);
criterion_main!(benches);
```

**주의**:
- `Browser`, `BrowserConfig`이 public API인지 확인
- `new_page()` 반환 타입이 `Arc<RwLock<Session>>` 인지 `Session` 인지 확인
- `session.write()` 가 async인지 확인 → `to_async` 필요

```bash
grep "pub fn new_page\|pub async fn new_page" crates/oxibrowser-core/src/browser.rs
grep "pub fn evaluate_js\|pub async fn evaluate_js" crates/oxibrowser-core/src/session.rs
```

#### Cargo.toml [dev-dependencies] 확인:

`crates/oxibrowser-core/Cargo.toml`:
```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio"] }
tokio = { workspace = true }

[[bench]]
name = "core_bench"
harness = false
```

이미 존재하는지 확인:
```bash
grep -A3 "\[dev-dependencies\]\|criterion\|\[bench\]" crates/oxibrowser-core/Cargo.toml
```

---

### Task 5: P3.3 — Memory Usage Benchmark

**목표**: 세션당 메모리 < 50MB 측정

**파일**: `crates/oxibrowser-core/benches/core_bench.rs`

Task 4의 같은 파일에 추가:

```rust
fn bench_session_memory(c: &mut Criterion) {
    c.bench_function("session_memory_overhead", |b| {
        b.iter(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let browser = rt.block_on(Browser::new(BrowserConfig::default())).unwrap();
            let _session = rt.block_on(browser.new_page("data:text/html,<p>test</p>"));

            #[cfg(target_os = "macos")]
            {
                // macOS: 사용할 수 있는 방법
                // 1. `/usr/bin/time -l` 로 외부 측정
                // 2. jemalloc stats
                // 3. rustc -Z print-type-sizes
                println!("Session created (measure externally with /usr/bin/time -l)");
            }

            #[cfg(target_os = "linux")]
            {
                let status = std::fs::read_to_string("/proc/self/status").unwrap();
                for line in status.lines() {
                    if line.starts_with("VmRSS:") {
                        let kb: usize = line.split_whitespace().nth(1).unwrap().parse().unwrap();
                        println!("VmRSS: {} KB ({} MB)", kb, kb / 1024);
                    }
                }
            }

            rt.block_on(browser.close()).unwrap();
        })
    });
}
```

벤치마크 그룹에 등록:
```rust
criterion_group!(
    benches,
    bench_browser_startup,
    bench_session_navigate,
    bench_js_eval,
    bench_session_memory
);
criterion_main!(benches);
```

---

## 검증 순서

각 Task 완료 후:

```bash
# 1. 빌드 확인
cargo build --workspace

# 2. 기존 테스트 회귀 확인
cargo test --workspace

# 3. Clippy
cargo clippy --workspace --all-targets -- -D warnings

# 4. 커밋
git add -A
git commit -m "feat(cdp): <task description>"
```

Task 3 (binary size) 후:
```bash
cargo build --release
ls -lh target/release/oxibrowser
# 타겟: < 10MB
```

Task 4/5 (benchmarks) 후:
```bash
cargo bench --bench core_bench
# 타겟: browser_startup < 50ms, session_navigate < 100ms
```

---

## 금지 사항

- `crates/oxibrowser-core/src/js/runtime.rs` 수정하지 말 것
- `crates/oxibrowser-core/src/session.rs` 수정하지 말 것 (import 제외)
- `crates/oxibrowser-core/src/frame.rs` 수정하지 말 것
- `crates/oxibrowser-core/src/page.rs` 수정하지 말 것
- `crates/oxibrowser-webapi/` 수정하지 말 것
