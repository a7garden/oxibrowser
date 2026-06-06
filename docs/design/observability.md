# OxiBrowser Observability 설계

> **목표:** OxiBrowser를 프로덕션에서 운영할 때 "무슨 일이 일어나고 있는지" 즉시 파악할 수 있게 만든다.
> 로깅·메트릭·트레이싱 3축을 일관된 모델로 통합한다.

---

## 1. 설계 원칙

| # | 원칙 | 설명 |
|---|------|------|
| P1 | **Zero-cost when disabled** | 관측성 비활성화 시 런타임 오버헤드 0. 컴파일 타임 feature flag로 완전 제거 |
| P2 | **Structured-first** | `format!` 문자열 결합 금지. 모든 로그는 `key = value` 구조화 필드 |
| P3 | **Span hierarchy = object hierarchy** | Browser → Session → Page → Frame 계층이 그대로 span 부모-자식 관계가 됨 |
| P4 | **Metrics via `tracing`** | 별도 metrics 크레이트 없이 `tracing` 이벤트 → `MetricsLayer` 로 카운터/히스토그램 추출 |
| P5 | **Opt-in progressive** | Phase 1은 `tracing` 만으로 달성. OTLP/Prometheus export는 Phase 2에서 추가 |

---

## 2. 아키텍처 개요

```
┌─────────────────────────────────────────────────────────┐
│                     OxiBinary (CLI)                      │
│  tracing_subscriber::fmt() + EnvFilter                  │
│  + MetricsLayer (Layer trait)                           │
├─────────────────────────────────────────────────────────┤
│                   oxibrowser-core                        │
│                                                          │
│  Browser ──[info_span!("browser")]──┐                   │
│    Session ──[info_span!("session")]──┼─ navigate()     │
│      Page ──[debug_span!("page")]──┤   evaluate_js()   │
│        Frame ──[debug_span!("frame")]  fetch()          │
│                                                          │
│  HttpClient ──[debug_span!("http")]── fetch/POST        │
│  JsRuntime  ──[debug_span!("js")]─── eval              │
├─────────────────────────────────────────────────────────┤
│                   oxibrowser-cdp                         │
│  CdpServer ──[info_span!("cdp.server")]                 │
│  CdpSession ──[info_span!("cdp.session")]               │
│    Domain handlers ──[debug_span!("cdp.command")]       │
└─────────────────────────────────────────────────────────┘
         │                    │                   │
         ▼                    ▼                   ▼
   stderr (fmt)         MetricsLayer        OTLP Exporter
   RUST_LOG 필터        (in-process)       (Phase 2, opt-in)
                              │
                              ▼
                      Atomic counters
                      + histograms
                      (lock-free)
```

---

## 3. Span 모델 — 핵심

### 3.1 Span 계층

```
browser:{id}
  ├── session:{id}
  │     ├── navigate:{url}
  │     │     └── http_fetch:{url}          ← HttpClient.fetch()
  │     ├── evaluate_js:{expr_truncated}
  │     └── post:{url}
  ├── cdp.server:{addr}
  │     └── cdp.session:{session_id}
  │           └── cdp.command:{method}      ← Page.navigate, Runtime.evaluate, ...
  └── tab:{tab_id}
        └── goto:{url}
```

### 3.2 Span 정의 방식

`#[instrument]` 속성을 사용. 수동 `span!()`은 `#[instrument]`를 붙일 수 없는
`impl` 블록 밖 함수나 `async` 클로저에만 사용.

```rust
// ✅ 기본 — 모든 인자가 자동으로 필드가 됨
#[tracing::instrument(skip(self), fields(otel.name = "session.navigate"))]
pub async fn navigate(&mut self, url: &str) -> Result<()> { ... }

// ✅ 타겟 ID 등 컨텍스트 필드 추가
#[tracing::instrument(
    skip(self, ws_stream),
    fields(session_id = %session_id),
)]
pub async fn new(ws_stream: WsStream, browser: Arc<Browser>) -> Result<Self> { ... }

// ✅ 에러를 span 이벤트로 기록 (에러 로그 중복 방지)
#[tracing::instrument(skip(self), err)]
pub async fn fetch(&self, url: &Url) -> Result<Response> { ... }
```

---

## 4. 로깅(Callsite) 상세 설계

### 4.1 레벨 가이드라인

| 레벨 | 용도 | 예시 |
|------|------|------|
| `error!` | 복구 불가, 즉시 조치 필요 | JS 런타임 크래시, 리스너 바인드 실패 |
| `warn!` | degraded, 조사 필요 | SSRF 차단, 세션 한도 도달, 리다이렉트 루프 |
| `info!` | 수명주기 전환 | browser 생성/종료, 세션 생성/종료, CDP 연결/해제 |
| `debug!` | 작업 단위 시작/완료 | HTTP 요청/응답, JS 평가, DOM 변경 적용 |
| `trace!` | 내부 디버깅 상세 | 쿠키 저장, mpsc 메시지 송수신, DOM 트리 순회 |

### 4.2 구조화 필드 규약

```rust
// ❌ 기존 — 문자열 포맷팅
tracing::warn!("continueRequest: unknown requestId={}", request_id);

// ✅ 목표 — 구조화 필드
tracing::warn!(request_id, "unknown requestId in continueRequest");
```

**표준 필드 이름 (전체 코드베이스 통일):**

| 필드 | 타입 | 사용 위치 |
|------|------|-----------|
| `browser` | `BrowserId` | Browser 수명주기 |
| `session_id` | `SessionId \| String` | Session, CDP session |
| `tab_id` | `Uuid` | Tab, BrowserEvent |
| `page_id` | `PageId` | Page |
| `frame_id` | `FrameId` | Frame |
| `url` | `&str \| Url` | navigate, fetch |
| `status` | `u16` | HTTP 응답 |
| `method` | `&str` | CDP 명령, HTTP 메서드 |
| `error` | `Display` | 에러 컨텍스트 |
| `duration_ms` | `u64` | 작업 소요 시간 |
| `request_id` | `String` | Fetch domain, Network |
| `peer` | `SocketAddr` | CDP 연결 |
| `attempt` | `u32` | 재시도 |
| `bytes` | `usize` | 전송량 |

### 4.3 파일별 로깅 계획

#### `oxibrowser-core/src/browser.rs` (현재 2개 → 목표 6개)

```rust
// 생성
info!(id = %self.id, "browser created");

// 세션 생성
info!(id = %self.id, session_count, "new session created");

// Tab 생성
info!(id = %self.id, tab_id = %tab_id, session_count, "new tab created");

// 쿠키 로드/저장 (이미 있음 — 구조화 필드로 정리)
info!(path = %path.display(), "loaded cookies from file");

// 종료 (이미 있음)
info!("browser closed");

// Drop 경고 (이미 있음)
warn!("browser dropped without explicit close");
```

#### `oxibrowser-core/src/session.rs` (현재 6개 → 목표 18개)

```rust
// navigate
info!(url = %parsed, "navigating");                          // 이미 있음
debug!(status, final_url = %final_url, duration_ms, "page fetched");
debug!(html_bytes = html.len(), "response decoded");

// navigate_with_retry
info!(attempt, max_retries, delay_ms, "retrying navigation"); // 이미 있음

// evaluate_js
debug!(expr_len = expression.len(), await = await_promise, "evaluating JS");
debug!(mutations = mutations.len(), "DOM mutations applied");
trace!(mutation = ?m, "applying DOM mutation");              // NEW

// post
info!(url = %parsed, content_type, "POST request");          // 이미 있음

// load_sub_resources
debug!(total = resource_urls.len(), "loading sub-resources");
info!(loaded, total, "sub-resources loaded");                // 이미 있음

// close
info!(id = %self.id, "session closed");                      // 이미 있음

// HTTP 응답 저장
trace!(request_id, body_len = html.len(), "response body stored");
```

#### `oxibrowser-core/src/network/client.rs` (현재 1개 → 목표 10개)

```rust
// fetch
debug!(url = %url, "HTTP request started");
debug!(url = %url, status, duration_ms, "HTTP response received");
trace!(url = %url, cookie_count, "cookies attached to request");
trace!(url = %url, set_cookie_count, "response cookies stored");

// SSRF
warn!(url = %url, host, "SSRF blocked: redirect to blocked IP"); // 이미 있음
warn!(host, "SSRF blocked: hostname resolves to blocked IP");

// intercept
debug!(url = %effective_url, method = effective_method, "intercepted request continued");
debug!(status_code, "intercepted request fulfilled");
```

#### `oxibrowser-core/src/js/runtime.rs` (현재 0개 → 목표 8개)

```rust
// evaluate
debug!(expr_len = expression.len(), timeout_ms, "JS evaluation started");
debug!(expr_len, has_value = result.value.is_some(), exception = ?result.exception.is_some(), duration_ms, "JS evaluation completed");
warn!(timeout_ms, "JS evaluation timed out — context reset");

// DOM snapshot injection
debug!(node_count, "DOM snapshot injected into JS runtime");

// console.log 캡처
trace!(output = ?line, "JS console output captured");
```

#### `oxibrowser-cdp/src/server.rs` (현재 9개 → 구조화만 정리)

기존 로그가 이미 구조화 필드를 잘 사용하고 있음. 정리만 필요:
- `info!(addr, peer)` → 유지
- `warn!(peer, error)` → 유지
- `error!(error)` → 유지

#### `oxibrowser-cdp/src/session.rs` (현재 ~5개 → 목표 10개)

```rust
// 세션 수명주기
info!(session_id, "CDP session created");                    // 이미 있음
info!(session_id, "CDP session started");                    // 이미 있음
info!(session_id, "CDP session ended");                      // 이미 있음

// 명령 처리
debug!(id, method, "dispatching CDP command");               // 이미 있음
debug!(text_len = text.len(), "sending CDP response");

// 에러
warn!(error, "CDP session error");                           // 이미 있음
warn!(size, max, "CDP message too large, dropping");         // 이미 있음
```

#### `oxibrowser-cdp/src/domains/*.rs` (구조화만 정리)

```rust
// fetch.rs — 기존 문자열 포맷 → 구조화 필드
tracing::warn!(request_id, "unknown requestId in continueRequest");
tracing::debug!(request_id, "Fetch.continueRequest resumed");
tracing::debug!(request_id, status_code, body_len, "Fetch.fulfillRequest completed");

// input.rs — 동일
tracing::debug!(text, "Input.insertText");
```

---

## 5. 메트릭 설계

### 5.1 접근법: `tracing` → `MetricsLayer`

별도 `metrics` 크레이트를 도입하지 않고, `tracing_subscriber::Layer` 트레이트를
구현한 `MetricsLayer`가 특정 span/event를 가로채서 lock-free 카운터를 업데이트.

**장점:**
- 의존성 최소화 (`tracing`만 필요)
- `RUST_LOG=off` 여도 메트릭은 동작 (Layer는 필터와 독립)
- 나중에 Prometheus export를 `MetricsLayer::snapshot()` 하나로 추가 가능

### 5.2 메트릭 정의

| 이름 | 타입 | 라벨 | 설명 |
|------|------|------|------|
| `oxibrowser.pages_total` | Counter | `status` | 로드된 총 페이지 수 |
| `oxibrowser.page_duration_ms` | Histogram | — | 페이지 로드 소요 시간 |
| `oxibrowser.js_evaluations_total` | Counter | `has_error` | JS 평가 총 횟수 |
| `oxibrowser.js_duration_ms` | Histogram | — | JS 평가 소요 시간 |
| `oxibrowser.js_timeouts_total` | Counter | — | JS 타임아웃 횟수 |
| `oxibrowser.http_requests_total` | Counter | `method`, `status_class` | HTTP 요청 총 수 |
| `oxibrowser.http_duration_ms` | Histogram | `method` | HTTP 요청 소요 시간 |
| `oxibrowser.http_bytes_received` | Counter | — | 수신 바이트 총량 |
| `oxibrowser.active_sessions` | Gauge | — | 현재 활성 세션 수 |
| `oxibrowser.active_tabs` | Gauge | — | 현재 활성 탭 수 |
| `oxibrowser.cdp_connections_total` | Counter | — | CDP 연결 총 수 |
| `oxibrowser.cdp_active_connections` | Gauge | — | 현재 활성 CDP 연결 |
| `oxibrowser.cdp_commands_total` | Counter | `method` | CDP 명령 총 수 |
| `oxibrowser.cdp_command_duration_ms` | Histogram | `method` | CDP 명령 소요 시간 |
| `oxibrowser.dom_mutations_total` | Counter | `type` | DOM 뮤테이션 총 수 |
| `oxibrowser.errors_total` | Counter | `kind` | CoreError 유형별 에러 수 |

### 5.3 MetricsLayer 구현 스케치

```rust
// crates/oxibrowser-core/src/observability/metrics.rs

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing_subscriber::Layer;

/// Lock-free 메트릭 카운터 모음.
#[derive(Debug)]
pub struct Metrics {
    pub pages_total: AtomicU64,
    pub page_duration_ns: AtomicU64,       // 나노초 합산 (평균 계산용)
    pub js_evaluations_total: AtomicU64,
    pub js_timeouts_total: AtomicU64,
    pub http_requests_total: AtomicU64,
    pub active_sessions: AtomicU64,
    pub active_tabs: AtomicU64,
    pub cdp_connections_total: AtomicU64,
    pub cdp_active_connections: AtomicU64,
    pub errors_total: AtomicU64,
    // Histogram은 배열로 버킷 구현 (나중에)
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pages_total: AtomicU64::new(0),
            page_duration_ns: AtomicU64::new(0),
            js_evaluations_total: AtomicU64::new(0),
            js_timeouts_total: AtomicU64::new(0),
            http_requests_total: AtomicU64::new(0),
            active_sessions: AtomicU64::new(0),
            active_tabs: AtomicU64::new(0),
            cdp_connections_total: AtomicU64::new(0),
            cdp_active_connections: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
        })
    }
}

/// `tracing_subscriber::Layer` 구현.
/// 특정 span 이름을 감시하여 on_close 시 카운터를 업데이트.
pub struct MetricsLayer {
    metrics: Arc<Metrics>,
}

impl<S> Layer<S> for MetricsLayer
where
    S: tracing_subscriber::layer::SubscriberExt,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        // 에러 카운팅
        if meta.level() == &tracing::Level::ERROR {
            self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn on_close(
        &self,
        id: tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(&id) {
            match span.name() {
                "navigate" | "goto" => {
                    self.metrics.pages_total.fetch_add(1, Ordering::Relaxed);
                    // duration은 span extension에서 읽기
                }
                "evaluate_js" => {
                    self.metrics.js_evaluations_total.fetch_add(1, Ordering::Relaxed);
                }
                "http_fetch" => {
                    self.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
                }
                "cdp.session" => {
                    self.metrics.cdp_connections_total.fetch_add(1, Ordering::Relaxed);
                    self.metrics.cdp_active_connections.fetch_sub(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
    }
}
```

### 5.4 메트릭 노출

**Phase 1 (CLI 모드):** `SIGUSR2` 또는 `--metrics` 플래그로 stderr에 JSON 출력.

```json
{
  "pages_total": 42,
  "page_duration_avg_ms": 312,
  "js_evaluations_total": 156,
  "js_timeouts_total": 2,
  "http_requests_total": 87,
  "active_sessions": 1,
  "cdp_active_connections": 0,
  "errors_total": 3
}
```

**Phase 2 (serve 모드):** `GET /metrics` 엔드포인트에서 Prometheus 포맷 노출.

---

## 6. CDP 헬스체크 엔드포인트

```rust
// server.rs handle_http_request에 추가
"/health" => {
    let metrics = /* MetricsLayer에서 참조 */;
    Ok(Response::builder()
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(serde_json::json!({
            "status": "ok",
            "active_sessions": metrics.active_sessions.load(Ordering::Relaxed),
            "active_cdp_connections": metrics.cdp_active_connections.load(Ordering::Relaxed),
            "uptime_secs": start_time.elapsed().as_secs(),
            "version": env!("CARGO_PKG_VERSION"),
        }).to_string())))
        ?)
}
```

---

## 7. 구현 파일 구조

```
crates/oxibrowser-core/src/
  observability/
    mod.rs              ← 공개 API + 초기화 헬퍼
    metrics.rs          ← Metrics 구조체 + MetricsLayer
    span_ext.rs         ← span에 duration 기록하는 헬퍼 트레이트

crates/oxibrowser-core/src/lib.rs
  + pub mod observability;
```

### `observability/mod.rs`

```rust
//! Observability infrastructure for OxiBrowser.
//!
//! Provides structured logging, span hierarchy, and metrics collection
//! all built on top of the `tracing` ecosystem.

pub mod metrics;
pub mod span_ext;

pub use metrics::{Metrics, MetricsLayer};

use metrics::MetricsLayer;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

/// Initialize the observability stack.
///
/// Call once at program start (before any `tracing` macros).
/// Returns the `Arc<Metrics>` handle for `/health` endpoint etc.
pub fn init() -> Arc<Metrics> {
    let metrics = Metrics::new();
    let metrics_layer = MetricsLayer::new(metrics.clone());

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn"));

    let subscriber = Registry::default()
        .with(filter)
        .with(tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
        )
        .with(metrics_layer);

    tracing::subscriber::set_global_default(subscriber)
        .expect("failed to set tracing subscriber");

    metrics
}
```

---

## 8. `#[instrument]` 적용 계획 — 파일별

| 파일 | 함수 | span 이름 | skip | fields |
|------|------|-----------|------|--------|
| `browser.rs` | `new()` | `"browser"` | `config` | — |
| `browser.rs` | `new_session()` | `"new_session"` | `self` | `browser = %self.id` |
| `browser.rs` | `browse()` | `"browse"` | `self` | `url` |
| `browser.rs` | `new_tab()` | `"new_tab"` | `self` | — |
| `browser.rs` | `close()` | `"browser_close"` | `self` | `id = %self.id` |
| `session.rs` | `navigate()` | `"navigate"` | `self` | `session = %self.id` |
| `session.rs` | `navigate_with_retry()` | `"navigate_retry"` | `self` | `url, max_retries` |
| `session.rs` | `evaluate_js_with_await()` | `"evaluate_js"` | `self` | `session = %self.id` |
| `session.rs` | `post()` | `"post"` | `self` | `url, content_type` |
| `session.rs` | `close()` | `"session_close"` | `self` | `id = %self.id` |
| `network/client.rs` | `fetch()` | `"http_fetch"` | `self` | `url` |
| `network/client.rs` | `intercept()` | `"http_intercept"` | `self, action` | `url` |
| `network/client.rs` | `post()` | `"http_post"` | `self, body` | `url` |
| `network/client.rs` | `post_json()` | `"http_post_json"` | `self, json` | `url` |
| `page.rs` | `from_html()` | `"page_create"` | — | `url, status` |
| `frame.rs` | `from_html()` | `"frame_create"` | — | `url` |
| `cdp/server.rs` | `start()` | `"cdp.server"` | `self` | `addr` |
| `cdp/session.rs` | `new()` | `"cdp.session.create"` | `ws_stream` | `session_id` |
| `cdp/session.rs` | `run()` | `"cdp.session.run"` | `self` | `session_id` |
| `cdp/session.rs` | `handle_text_message()` | `"cdp.command"` | `self` | `method` |

---

## 9. `Display` 구현 — ID 타입들

이미 `BrowserId`, `SessionId`, `PageId`, `FrameId`가 `Display`를 구현하고 있음.
tracing 매크로에서 `%id`로 자동 사용 가능. 변경 불필요.

---

## 10. 구현 순서 (Phase 1)

### Step 1: `observability` 모듈 생성 + `MetricsLayer`
- `crates/oxibrowser-core/src/observability/{mod,metrics,span_ext}.rs`
- `Cargo.toml` 의존성 변경 없음 (`tracing`, `tracing-subscriber`만 사용)

### Step 2: `main.rs` 초기화 교체
- 기존 `tracing_subscriber::fmt()...init()` → `observability::init()`

### Step 3: 핵심 경로에 `#[instrument]` 적용
- `browser.rs`: `new`, `new_session`, `browse`, `new_tab`, `close`
- `session.rs`: `navigate`, `evaluate_js_with_await`, `post`, `close`
- `network/client.rs`: `fetch`, `intercept`, `post*`

### Step 4: 기존 로그 구조화 필드로 전환
- `fetch.rs`, `input.rs`의 `format!` → `key = value`

### Step 5: `trace!` 레벨 로그 추가
- `network/client.rs`: 쿠키 저장/조회
- `session.rs`: DOM 뮤테이션 적용
- `js/runtime.rs`: console 캡처

### Step 6: `/health` 엔드포인트 (serve 모드)
- `cdp/server.rs`에 라우팅 추가
- `Metrics` 스냅샷 반환

### Step 7: 테스트
- 각 span이 올바르게 생성되는지 확인하는 단위 테스트
- `MetricsLayer` 카운터 증가 확인하는 통합 테스트

---

## 11. Phase 2 (이후)

| 항목 | 설명 |
|------|------|
| **OpenTelemetry Export** | `tracing-opentelemetry` + `opentelemetry-otlp` 로 Jaeger/Tempo에 트레이스 전송 |
| **Prometheus `/metrics`** | `MetricsLayer::snapshot()` → Prometheus 포맷 렌더링 |
| **Histogram 버킷** | `page_duration_ms`, `js_duration_ms` 등에 p50/p95/p99 분포 |
| **Structured Error Events** | `CoreError`에 `error_kind` 라벨 부여하여 `errors_total{kind="JsTimeout"}` |
| **CDP `OXI.getMetrics`** | 외부 클라이언트(Puppeteer/Playwright)가 메트릭을 쿼리할 수 있는 CDP 익스텐션 |
| **Structured Diagnostics** | `SIGUSR2` → 현재 활성 span 트리를 stderr에 덤프 |

---

## 12. 의존성 변화

### Phase 1 — 추가 없음

```toml
# 기존 그대로. 새 크레이트 불필요.
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

### Phase 2 — OTLP/Prometheus

```toml
# 추가
tracing-opentelemetry = "0.28"
opentelemetry = "0.27"
opentelemetry-otlp = "0.27"
opentelemetry-stdout = "0.27"  # 개발용
```

---

## 13. 코드 예시 — Before/After

### Before (browser.rs)

```rust
pub async fn new(config: BrowserConfig) -> Result<Self> {
    // ... setup ...
    info!(id = %id, "browser created");
    Ok(Self { id, ... })
}
```

### After (browser.rs)

```rust
#[tracing::instrument(skip(config), fields(id = %BrowserId::next_preview()))]
pub async fn new(config: BrowserConfig) -> Result<Self> {
    // ... setup ...
    tracing::Span::current().record("id", tracing::field::display(id));
    Ok(Self { id, ... })
}
```

> **참고:** `BrowserId::next_preview()`는 아직 ID가 확정되지 않았을 때 임시 값을
> 반환하는 헬퍼. 실제 ID는 함수 본문에서 `record()`로 덮어씀.
> 더 간단한 방법은 ID를 함수 안에서 생성하고 span 밖에서 생성하는 것.

**더 현실적인 접근 — ID를 미리 생성:**

```rust
pub async fn new(config: BrowserConfig) -> Result<Self> {
    let id = BrowserId::next();
    let _span = tracing::info_span!("browser", %id).entered();
    // ... 기존 코드 ...
}
```

### Before (session.rs navigate)

```rust
pub async fn navigate(&mut self, url: &str) -> Result<()> {
    let parsed = Url::parse(url)?;
    info!(url = %parsed, "navigating");
    let response = self.http_client.fetch(&parsed).await?;
    // ... 기존 코드 ...
}
```

### After (session.rs navigate)

```rust
#[tracing::instrument(
    skip(self),
    fields(session = %self.id),
    err
)]
pub async fn navigate(&mut self, url: &str) -> Result<()> {
    let parsed = Url::parse(url)?;
    let start = Instant::now();
    let response = self.http_client.fetch(&parsed).await?;
    let status = response.status().as_u16();
    let final_url = response.url().clone();
    debug!(status, final_url = %final_url, elapsed_ms = start.elapsed().as_millis() as u64, "page fetched");
    // ... 기존 코드 ...
}
```

### Before (fetch.rs CDP domain)

```rust
tracing::warn!("continueRequest: unknown requestId={}", request_id);
tracing::debug!("Fetch.continueRequest: requestId={} resumed", request_id);
```

### After (fetch.rs CDP domain)

```rust
tracing::warn!(request_id, "unknown requestId in continueRequest");
tracing::debug!(request_id, "Fetch.continueRequest resumed");
```

---

## 14. 검증 체크리스트

구현 완료 후 다음을 확인:

- [ ] `RUST_LOG=warn` (기본) → 기존처럼 warn/error만 출력
- [ ] `RUST_LOG=oxibrowser_core=debug` → navigate, fetch, JS eval 디버그 로그
- [ ] `RUST_LOG=oxibrowser_core=trace` → 쿠키, DOM 뮤테이션, 콘솔 캡처 상세
- [ ] span 계층이 `browser > session > navigate > http_fetch` 로 중첩되는지 확인
  - `RUST_LOG=debug` + `tracing_subscriber::fmt().with_target(true)`로 검증
- [ ] `Metrics` 카운터가 navigate/evaluate_js 호출 후 증가하는지 단위 테스트
- [ ] 기존 테스트 전체 통과 (`cargo test --workspace`)
- [ ] 새 의존성 0개 (Phase 1)
- [ ] 바이너리 크기 변화 < 1%
