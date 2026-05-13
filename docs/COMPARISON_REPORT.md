# OxiBrowser vs Lightpanda — 심층 비교 분석 보고서

> **작성일:** 2026-05-14  
> **분석 대상:** OxiBrowser (Rust, ~10,623 LOC) vs Lightpanda (Zig, ~123,481 LOC)  
> **분석 방법:** 5개 병렬 서브에이전트로 전체 코드베이스 분석 (코어 아키텍처, CDP, JS/WebAPI, 네트워크/스토리지, OxiBrowser 심층 분석)

---

## 목차

1. [요약](#1-요약)
2. [규모 및 성숙도 비교](#2-규모-및-성숙도-비교)
3. [아키텍처 비교](#3-아키텍처-비교)
4. [JS 엔진 비교](#4-js-엔진-비교)
5. [CDP 구현 비교](#5-cdp-구현-비교)
6. [WebAPI / DOM 비교](#6-webapi--dom-비교)
7. [네트워크 스택 비교](#7-네트워크-스택-비교)
8. [메모리 관리 비교](#8-메모리-관리-비교)
9. [동시성 모델 비교](#9-동시성-모델-비교)
10. [보안 기능 비교](#10-보안-기능-비교)
11. [테스트 및 품질 비교](#11-테스트-및-품질-비교)
12. [코드 레벨 상세 비교](#12-코드-레벨-상세-비교)
13. [Lightpanda에서 배울 점](#13-lightpanda에서-배울-점)
14. [OxiBrowser만의 장점](#14-oxibrowser만의-장점)
15. [실행 로드맵 제안](#15-실행-로드맵-제안)

---

## 1. 요약

| 항목 | OxiBrowser | Lightpanda |
|------|-----------|------------|
| **언어** | Rust (Edition 2021) | Zig (0.15.2+) |
| **LOC** | ~10,623줄 | ~123,481줄 (**11.6x** 더 큼) |
| **소스 파일** | 38개 Rust 파일 | 376개 Zig 파일 |
| **크레이트/모듈** | 4개 크레이트 | 단일 프로젝트 |
| **JS 엔진** | boa_engine (순수 Rust, ES2024+) | V8 (C++ FFI, 완전한 ECMAScript) |
| **HTML 파서** | html5ever (직접 Rust) | html5ever (Rust→C FFI→Zig) |
| **HTTP 클라이언트** | reqwest (Rust) | libcurl (C FFI) |
| **WebSocket** | tokio-tungstenite | 커스텀 구현 (RFC 6455) |
| **CDP 도메인** | 7개 | 20개 (19 CDP + 1 벤더) |
| **CDP 메서드** | ~35개 | ~100개 |
| **WebAPI 타입** | ~15개 | **160+개** (67개 HTML 요소) |
| **라이선스** | MIT | AGPL-3.0 |

**핵심 차이:** Lightpanda는 11.6배 더 큰 코드베이스로, 프로덕션급 헤드리스 브라우저입니다. V8 + 완전한 WebAPI + 미들웨어 네트워크 스택을 갖추고 있어 Puppeteer/Playwright 완벽 호환을 목표로 합니다. OxiBrowser는 초기 단계로, 아키텍처 틀은 훌륭하지만 기능 구현이 많이 필요합니다.

---

## 2. 규모 및 성숙도 비교

### 2.1 코드 규모

| 컴포넌트 | OxiBrowser LOC | Lightpanda LOC | 비율 |
|----------|---------------|----------------|------|
| 코어 (Browser/Session/Page/Frame) | ~960 | ~2,500 | 1:2.6 |
| JS 런타임 | ~2,950 | ~3,500 | 1:1.2 |
| CDP 서버 + 도메인 | ~2,797 | ~12,000 | 1:4.3 |
| WebAPI / DOM | ~1,271 | ~35,000 | 1:27.5 |
| 네트워크 | ~410 | ~4,000 | 1:9.8 |
| 스토리지 | 0 | ~1,200 | 0:1 |
| CLI | ~761 | ~800 | 1:1.0 |
| 테스트 | ~480 (e2e) | ~8,000+ | 1:16.7 |
| **총계** | **~10,623** | **~123,481** | **1:11.6** |

### 2.2 기능 완성도 매트릭스

| 기능 | OxiBrowser | Lightpanda | 격차 |
|------|:----------:|:----------:|------|
| HTML 파싱 | ✅ | ✅ | 동등 |
| CSS 선택자 | ✅ (기본) | ✅ (완전) | 보통 |
| XPath | ❌ | ✅ (완전 1.0) | 큼 |
| JS 실행 | ✅ (boa_engine) | ✅ (V8) | 다름 |
| ES 모듈 | ❌ | ✅ | 큼 |
| DOM 조작 | ⚠️ (일부) | ✅ (완전) | 큼 |
| 이벤트 시스템 | ⚠️ (no-op) | ✅ (capture/bubble) | 큼 |
| CSS 파싱 | ❌ | ✅ | 큼 |
| Web Worker | ❌ | ✅ | 큼 |
| WebSocket (클라이언트) | ❌ | ✅ (RFC 6455) | 큼 |
| Fetch API | ⚠️ (스텁) | ✅ (완전) | 큼 |
| XHR | ❌ | ✅ | 큼 |
| Web Crypto | ❌ | ✅ (AES/EC/HMAC/RSA) | 큼 |
| Streams API | ❌ | ✅ | 큼 |
| Canvas | ❌ | ✅ (2D) | 큼 |
| Storage (localStorage) | ⚠️ (메모리만) | ✅ (SQLite) | 보통 |
| HTTP 캐시 | ❌ | ✅ (FsCache) | 큼 |
| Robots.txt | ❌ | ✅ (RFC 9309) | 큼 |
| IP 필터링 | ❌ | ✅ (CIDR) | 큼 |
| Web Bot Auth | ❌ | ✅ (Ed25519) | 큼 |
| MCP 서버 | ❌ | ✅ | 보통 |
| 접근성 트리 | ❌ | ✅ (완전) | 큼 |
| 구조화된 데이터 추출 | ❌ | ✅ (JSON-LD, OG, Twitter) | 보통 |
| 인터랙티브 요소 스캔 | ❌ | ✅ | 보통 |
| 인코딩 감지 | ✅ | ✅ | 동등 |
| 쿠키 관리 | ✅ | ✅ | 동등 |
| 스크린샷 | ⚠️ (1x1 PNG) | ⚠️ (임베드 이미지) | 동등 |
| PDF | ⚠️ (빈 데이터) | ⚠️ (임베드 PDF) | 동등 |

---

## 3. 아키텍처 비교

### 3.1 공통점: Browser → Session → Page → Frame 계층

두 프로젝트 모두 동일한 계층 구조를 사용합니다. 이는 Lightpanda의 설계를 OxiBrowser가 포팅한 것입니다.

```
┌─────────────────────────────────────────────────────────┐
│                   공통 아키텍처                          │
│                                                          │
│  Browser (싱글톤)                                       │
│  ├── 세션 목록 관리                                      │
│  ├── 전역 쿠키 jar                                      │
│  ├── HTTP 클라이언트 공유                                │
│  └── 설정 (Config)                                      │
│       │                                                  │
│       └── Session (브라우징 컨텍스트)                    │
│           ├── 네비게이션 히스토리                        │
│           ├── 로컬 스토리지                              │
│           ├── JS 런타임                                  │
│           └── Page (로드된 문서)                         │
│               ├── 상태 코드, 콘텐츠 타입                 │
│               ├── 리소스 추적                            │
│               └── Frame (DOM + 자식 프레임)              │
│                   ├── HTML 파싱된 Document               │
│                   ├── 자식 Frame (iframe)                │
│                   └── DOM 버전 추적                      │
└─────────────────────────────────────────────────────────┘
```

### 3.2 핵심 아키텍처 차이점

#### 페이지 네비게이션 상태 기계

**Lightpanda**는 복잡한 Active/Pending 페이지 상태 기계를 사용합니다:

```
Lightpanda:
                        ┌─────────────────┐
                        │   No Page       │
                        └────────┬────────┘
                                 │ createPage()
                                 ▼
                        ┌─────────────────┐
                   ┌───▶│  Active Page    │◀───┐
                   │    └────────┬────────┘    │
                   │             │ initiateRootNavigation()
                   │             ▼
                   │    ┌─────────────────┐
                   │    │ Pending Page    │  ← HTTP 요청 진행 중
                   │    │ (이전 페이지 유지)│
                   │    └────────┬────────┘
                   │             │ commitPendingPage() [5단계]
                   │             ▼
                   │    ┌─────────────────┐
                   │    │ 포인터 교체 +   │
                   │    │ 이전 페이지 삭제 │
                   │    └─────────────────┘
                   └─────────────┘

OxiBrowser:
  Session.navigate(url)
    → fetch(url)
    → HTML 파싱
    → 새 Page 생성 (즉시 교체)
    → 이전 Page는 버려짐
```

Lightpanda의 접근 방식이 **CDP 호환성에 중요**합니다. HTTP 왕복 동안 이전 페이지가 살아있어야 Puppeteer/Playwright가 올바르게 동작합니다.

#### 크레이트 구조 vs 모놀리식

```
OxiBrowser (크레이트 분리):
┌──────────────┐     ┌──────────────┐
│  oxibrowser  │────▶│oxibrowser-cdp│
│  (binary)    │     │              │
└──────┬───────┘     └──────┬───────┘
       │                    │
       └──────┬─────────────┘
              ▼
       ┌──────────────┐     ┌───────────────┐
       │oxibrowser-core│───▶│oxibrowser-webapi│
       └──────────────┘     └───────────────┘

Lightpanda (모놀리식):
src/
├── lightpanda.zig  ← 루트 리익스포트
├── App.zig         ← 앱 싱글톤
├── browser/        ← 브라우저 로직
│   ├── Session.zig
│   ├── HttpClient.zig
│   ├── js/         ← V8 래퍼
│   ├── webapi/     ← DOM + WebAPI
│   └── parser/     ← HTML 파서
├── cdp/            ← CDP 서버
├── network/        ← 네트워크 스택
├── storage/        ← 스토리지
└── mcp/            ← MCP 서버
```

OxiBrowser의 크레이트 분리는 **컴파일 시간 최적화**와 **관심사 분리**에 유리합니다. Lightpanda는 Zig의 컴파일 타임 분석으로 모놀리식에서도 효율적입니다.

---

## 4. JS 엔진 비교

이것이 **가장 근본적인 기술적 차이**입니다.

### 4.1 엔진 선택

| 항목 | OxiBrowser (boa_engine) | Lightpanda (V8) |
|------|------------------------|-----------------|
| 구현 언어 | 순수 Rust | C++ (V8) |
| 바이너리 크기 | ~1-2MB | ~30-50MB (V8 포함) |
| C 의존성 | 없음 | V8, ICU |
| ECMAScript 준수 | ES2024+ | 완전 (ES2024+) |
| `Send` 안전 | ❌ (`!Send`, GC NonNull) | ❌ (V8 Isolate) |
| 스레딩 | 전용 OS 스레드 + mpsc 채널 | 전용 스레드 (V8 스레드 친화성) |
| 컨텍스트 생성 비용 | ~수백 μs | ~수 ms (스냅샷으로 최적화) |
| 스냅샷 지원 | 없음 | ✅ (빌드 타임 V8 스냅샷) |

### 4.2 아키텍처 패턴 비교

**OxiBrowser — 전용 스레드 + 채널:**
```rust
// JsRuntime은 Send + Sync이지만, Context는 !Send
// 해결: 전용 OS 스레드에서 Context 유지
pub struct JsRuntime {
    tx: mpsc::Sender<JsCommand>,    // 메인→JS
    rx: Mutex<mpsc::Receiver<JsResponse>>,  // JS→메인
    console: Arc<Mutex<Vec<String>>>,
    mutations: Arc<RwLock<Vec<DomMutation>>>,
    dom_snapshot: Arc<RwLock<Option<DomSnapshot>>>,
}

// JS 스레드:
fn js_thread_fn(rx, tx, context) {
    for cmd in rx.iter() {
        match cmd {
            Eval(expr) => {
                let result = context.eval(&expr);
                tx.send(EvalResult(result));
            }
            // ...
        }
    }
}
```

**Lightpanda — V8 Isolate + Inspector 위임:**
```zig
// CDP Runtime.evaluate가 V8 Inspector에 직접 위임
fn sendInspector(cmd: *CDP.Command, action: anytype) !void {
    const bc = cmd.browser_context orelse return error.BrowserContextNotLoaded;
    bc.callInspector(cmd.input.json);  // JSON 그대로 전달
}

// Inspector가 컨텍스트 생성/파괴, 리모트 객체 관리, 스크립트 컴파일/실행 모두 처리
```

### 4.3 DOM-JS 브릿지

**OxiBrowser — DomSnapshot 패턴:**
```rust
// DOM을 평면 HashMap으로 변환하여 JS에 주입
pub struct DomSnapshot {
    pub nodes: HashMap<u32, DomNode>,  // nodeId → 노드
    pub root_id: u32,
    pub body_id: Option<u32>,
    pub head_id: Option<u32>,
}

// JS에서 document.querySelector 등으로 접근
// 뮤테이션은 Arc<RwLock<Vec<DomMutation>>>으로 수집 후 Rust DOM에 적용
```

**Lightpanda — TaggedOpaque + Identity 패턴:**
```zig
// V8에 DOM 객체를 타입 태그가 있는 불투명 포인터로 전달
const TaggedOpaque = struct {
    prototype_len: u16,
    prototype_chain: [*]const PrototypeChainEntry,
    value: *anyopaque,     // 실제 Zig DOM 인스턴스
    subtype: ?SubType,
};

// Identity 맵으로 동일 노드 → 동일 JS 객체 보장 (=== 의미론)
const Identity = struct {
    identity_map: AutoHashMapUnmanaged(usize, v8.Global),
};
```

**분석:** Lightpanda의 접근이 더 견고합니다. TaggedOpaque는 프로토타입 체인을 통한 안전한 업캐스팅을 제공하고, Identity 맵은 `window.top.document === top's document` 같은 객체 동일성을 보장합니다. OxiBrowser의 DomSnapshot은 간단하지만, 깊은 DOM에서 메모리 오버헤드가 크고 동일성 보장이 없습니다.

### 4.4 WebAPI 노출

| WebAPI 카테고리 | OxiBrowser | Lightpanda |
|----------------|:----------:|:----------:|
| `document` 객체 | ✅ (쿼리, 속성) | ✅ (완전) |
| `window` 객체 | ✅ (기본) | ✅ (완전) |
| `console` | ✅ (log/warn/error/info) | ✅ (완전, 15+ 메서드) |
| `setTimeout/setInterval` | ⚠️ (동기 실행) | ✅ (비동기 스케줄러) |
| `fetch()` | ⚠️ (하드코딩 응답) | ✅ (완전, AbortSignal) |
| `XMLHttpRequest` | ❌ | ✅ |
| `WebSocket` | ❌ | ✅ (완전 RFC 6455) |
| `localStorage/sessionStorage` | ⚠️ (메모리) | ✅ (SQLite) |
| `Crypto.subtle` | ❌ | ✅ (AES/EC/HMAC/RSA/X25519) |
| `Streams` | ❌ | ✅ (Readable/Writable/Transform) |
| `Canvas` | ❌ | ✅ (2D) |
| `WebGL` | ❌ | ⚠️ (스텁) |
| `Worker` | ❌ | ✅ |
| ES Modules | ❌ | ✅ (동적 import 포함) |
| `EventTarget` | ⚠️ (no-op) | ✅ (capture/bubble/dispatch) |
| `MutationObserver` | ❌ | ✅ |
| `Custom Elements` | ❌ | ✅ |
| `Shadow DOM` | ❌ | ✅ |
| `XPath` | ❌ | ✅ (완전 1.0) |
| `Range/Selection` | ❌ | ✅ |
| `File/Blob` | ❌ | ✅ |
| `Encoding (TextEncoder/Decoder)` | ❌ | ✅ |
| `URL/URLSearchParams` | ❌ | ✅ |

---

## 5. CDP 구현 비교

### 5.1 도메인 커버리지

| 도메인 | OxiBrowser 메서드 | Lightpanda 메서드 | 격차 분석 |
|--------|:-----------------:|:-----------------:|-----------|
| **Browser** | 5 | 7 | `setPermission` 등 누락 |
| **Page** | ~10 | 16 | lifecycle 이벤트, scripts on new doc, isolated worlds |
| **DOM** | 6 | 15 | `performSearch`(XPath), `getBoxModel`, `resolveNode` |
| **Runtime** | 7 | 8 | `callFunctionOn`, `getProperties`는 V8 Inspector 위임 |
| **Network** | 5 | 11 | 쿠키 CRUD, `getResponseBody`, `setExtraHTTPHeaders` |
| **Fetch** | 6 | 6 | `continueWithAuth` 누락, 실제 인터셉션 안 됨 |
| **Target** | 8 | 13 | `sendMessageToTarget`, `createBrowserContext` |
| **CSS** | 0 | 1 | — |
| **Console** | 0 | 3 | — |
| **Inspector** | 0 | 2 | — |
| **Input** | 0 | 3 | 키보드/마우스 이벤트 |
| **Emulation** | 0 | 5 | UA 오버라이드, 미디어, 터치 |
| **Security** | 0 | 3 | TLS 인증서 에러 무시 |
| **Storage** | 0 | 3 | 쿠키 관리 (browserContextId) |
| **Accessibility** | 0 | 3 | 완전한 AX 트리 |
| **Audits** | 0 | 2 | — |
| **Performance** | 0 | 2 | — |
| **Log** | 0 | 2 | — |
| **LP (벤더)** | 0 | 12 | AI 에이전트 전용 (getMarkdown, clickNode 등) |
| **총계** | **~47** | **~100** | |

### 5.2 디스패치 최적화

**OxiBrowser — 런타임 문자열 매칭:**
```rust
pub fn dispatch(method: &str, params: Option<Value>, ctx: DispatchContext) -> DomainResult {
    let (domain, method_name) = method.split_once('.').unwrap();
    match domain {
        "Browser" => browser::handle(method_name, params, ctx),
        "Page" => page::handle(method_name, params, ctx),
        // ... 7개 도메인
        _ => Err(CdpError { code: -32601, message: format!("Unknown domain: {}", domain) }),
    }
}
```

**Lightpanda — 컴파일 타임 바이트 패턴 매칭:**
```zig
// 문자열 비교 대신 정수 캐스팅으로 제로 코스트 디스패치
switch (domain.len) {
    3 => switch (@as(u24, @bitCast(domain[0..3].*))) {
        asUint(u24, "DOM") => return dom.processMessage(command),
        asUint(u24, "CSS") => return css.processMessage(command),
        else => {},
    },
    7 => switch (@as(u56, @bitCast(domain[0..7].*))) {
        asUint(u56, "Browser") => return browser.processMessage(command),
        // ...
    },
}
```

**분석:** Lightpanda의 접근이 컴파일 타임에 최적화되어 있지만, OxiBrowser의 런타임 매칭도 충분히 빠릅니다 (7개 도메인만). Rust에서는 `phf` (완전 해시 함수)나 `const` 매칭으로 유사한 최적화가 가능합니다.

### 5.3 이벤트 시스템

**OxiBrowser — BroadcastChannel:**
```rust
// per-connection mpsc 채널 + AtomicBool 게이팅
pub struct EventSender {
    tx: mpsc::UnboundedSender<CdpEvent>,
    page_enabled: Arc<AtomicBool>,
    runtime_enabled: Arc<AtomicBool>,
    network_enabled: Arc<AtomicBool>,
    fetch_enabled: Arc<AtomicBool>,
}

// 이벤트 전송 시 플래그 확인
pub fn send_page_event(&self, method: &str, params: Value) {
    if self.page_enabled.load(Ordering::Relaxed) {
        self.send_event(method, params);
    }
}
```

**Lightpanda — 컴파일 타임 타입 안전 이벤트 버스:**
```zig
// 19개 이벤트 타입, 컴파일 타임에 타입 안전
pub fn register(self, comptime event: EventType, receiver: anytype, 
                func: EventFunc(event)) !void
pub fn dispatch(self, comptime event: EventType, data: ArgType(event)) void

// 도메인 enable/disable이 register/unregister 트리거
pub fn networkEnable(self) !void {
    try self.notification.register(.http_request_start, self, onHttpRequestStart);
    try self.notification.register(.http_request_done, self, onHttpRequestDone);
}
```

**분석:** Lightpanda의 접근이 더 정교합니다. enable/disable이 실제로 이벤트 핸들러를 등록/해제하여 불필요한 이벤트 생성을 원천 차단합니다. OxiBrowser는 이벤트를 생성하고 플래그로 필터링합니다.

### 5.4 알림(Notification) 스코핑

| 항목 | OxiBrowser | Lightpanda |
|------|-----------|------------|
| Notification 수명 | CdpSession (per WebSocket 연결) | BrowserContext (per CDP 연결) |
| 세션 간 격리 | ✅ (각 연결이 독립) | ✅ (per BrowserContext) |
| 이벤트 유형 | 하드코딩 (page/runtime/network/fetch) | 컴파일 타임 생성 (19개) |
| 등록/해제 | AtomicBool 플래그 | 실제 register/unregister |

---

## 6. WebAPI / DOM 비교

### 6.1 HTML 요소 구현

**OxiBrowser:** 제네릭 `NodeType::Element` 하나로 모든 요소 처리:
```rust
pub enum NodeType {
    Document,
    Element { tag: String, attributes: Vec<(String, String)> },
    Text(String),
    Comment(String),
    Doctype { name: String },
}
```

**Lightpanda:** 67개 개별 HTML 요소 타입:
```
Html, Head, Body, Div, Span, Paragraph, Pre, BR, HR, Generic, Unknown,
Base, Link, Meta, Script, Style, Title,
Form, Input, Button, Select, Option, OptGroup, TextArea, Label, Legend,
FieldSet, DataList, Output, Progress, Meter, ValidityState,
Table, TableCaption, TableCell, TableCol, TableRow, TableSection,
UL, OL, LI, DList,
Image, Audio, Video, Source, Track, Picture, Embed, Object, Param, Canvas, IFrame,
Anchor, Area, Details, Dialog, Map,
Heading, Quote, Time, Data, Mod, Font,
Template, Slot, Custom
```

**분석:** Lightpanda는 각 요소에 대해 요소 특화 WebAPI 속성과 메서드를 제공합니다 (예: `HTMLInputElement.value`, `HTMLAnchorElement.href`). OxiBrowser는 속성만으로 모든 것을 처리하여 간단하지만, 요소 특화 동작이 없습니다.

### 6.2 CSS 선택자 지원

| 선택자 | OxiBrowser | Lightpanda |
|--------|:----------:|:----------:|
| 태그 (`div`) | ✅ | ✅ |
| 클래스 (`.foo`) | ✅ | ✅ |
| ID (`#bar`) | ✅ | ✅ |
| 복합 (`div.foo`) | ✅ | ✅ |
| 속성 (`[href]`, `[a=b]`) | ✅ (DomSnapshot) | ✅ |
| 결합자 (`div > p`, `div p`) | ❌ | ✅ |
| 의사 클래스 (`:first-child`) | ❌ | ✅ |
| 의사 요소 (`::before`) | ❌ | ✅ |
| XPath | ❌ | ✅ (완전 1.0 구현) |

### 6.3 이벤트 처리

**OxiBrowser — no-op 스텁:**
```rust
// addEventListener, removeEventListener, dispatchEvent 모두 no-op
"addEventListener" => { /* no-op */ Ok(None) }
```

**Lightpanda — 완전한 캡처/버블 디스패치:**
```zig
// EventTarget.addEventListener with options: capture, bubble, once, passive
// EventManager.dispatch: 캡처 단계 → 타겟 단계 → 버블 단계
// 20개 이벤트 타입: MouseEvent, KeyboardEvent, FocusEvent, CustomEvent, ...
```

### 6.4 DOM 트리 조작

| 연산 | OxiBrowser | Lightpanda |
|------|:----------:|:----------:|
| `appendChild` | ✅ (초기 파싱만) | ✅ (완전) |
| `removeChild` | ❌ (no-op) | ✅ |
| `insertBefore` | ❌ | ✅ |
| `replaceChild` | ❌ | ✅ |
| `setAttribute` | ✅ | ✅ |
| `removeAttribute` | ❌ | ✅ |
| `cloneNode` | ❌ | ✅ |
| `importNode` | ❌ | ✅ |
| Shadow DOM | ❌ | ✅ |
| Custom Elements | ❌ | ✅ |

---

## 7. 네트워크 스택 비교

### 7.1 아키텍처 차이

```
OxiBrowser:
┌─────────────────────────────────────┐
│  reqwest::Client (Rust, 고수준)     │
│  ├── 연결 풀링 (자동)              │
│  ├── TLS (rustls)                   │
│  ├── 쿠키 (수동 주입)              │
│  └── 리다이렉트 (자동)              │
├─────────────────────────────────────┤
│  HttpClient (래퍼)                  │
│  ├── fetch_text()                   │
│  ├── post()                         │
│  └── cookie_jar (수동)              │
└─────────────────────────────────────┘

Lightpanda:
┌─────────────────────────────────────┐
│  libcurl multi handle (C, 저수준)   │
│  ├── 커스텀 이벤트 루프 (poll)     │
│  ├── 커넥션 풀 (수동 관리)         │
│  ├── TLS (BoringSSL)                │
│  ├── IP 필터 (CIDR)                 │
│  └── 커스텀 할당자                  │
├─────────────────────────────────────┤
│  미들웨어 체인:                     │
│  Interception → WebBotAuth →        │
│  Cache → Robots → [curl multi]      │
├─────────────────────────────────────┤
│  HTTP 캐시 (SHA256 키, 파일 시스템)│
│  Robots.txt (RFC 9309, 스레드 안전) │
│  IP 필터 (사설 IP 차단)            │
│  Web Bot Auth (Ed25519 서명)        │
└─────────────────────────────────────┘
```

### 7.2 기능 비교

| 기능 | OxiBrowser | Lightpanda |
|------|:----------:|:----------:|
| HTTP GET/POST | ✅ | ✅ |
| PUT/DELETE/PATCH | ❌ (reqwest는 지원하지만 래퍼 미구현) | ✅ |
| HTTP/2 | ✅ (reqwest 자동) | ✅ (libcurl) |
| TLS | ✅ (rustls) | ✅ (BoringSSL) |
| 쿠키 자동 관리 | ⚠️ (수동 주입/저장) | ✅ (per-session jar) |
| 연결 풀링 | ✅ (reqwest 자동) | ✅ (수동 관리) |
| 리다이렉트 추적 | ✅ | ✅ |
| 압축 (gzip/brotli) | ✅ (reqwest 자동) | ✅ (libcurl) |
| HTTP 캐시 | ❌ | ✅ (FsCache, RFC 9111) |
| Robots.txt | ❌ | ✅ (RFC 9309 완전) |
| IP 필터링 | ❌ | ✅ (사설 IP, 커스텀 CIDR) |
| Web Bot Auth | ❌ | ✅ (Ed25519) |
| 요청 인터셉션 | ⚠️ (알림만) | ✅ (실제 일시정지/수정/합성) |
| 프록시 지원 | ❌ | ✅ |
| HTTP 인증 | ❌ | ✅ (Basic/Digest) |
| WebSocket 클라이언트 | ❌ | ✅ (커스텀 RFC 6455) |
| IDN (국제화 도메인) | ❌ | ✅ (libidn2) |

### 7.3 미들웨어 패턴

Lightpanda의 **Layer 체인**은 OxiBrowser가 채택해야 할 핵심 패턴입니다:

```zig
// vtable 기반 타입 소거 인터페이스
pub const Layer = struct {
    ptr: *anyopaque,
    vtable: *const VTable,
    pub const VTable = struct {
        request: *const fn (*anyopaque, *Transfer) anyerror!void,
    },
};

// 체인: Interception → WebBotAuth → Cache → Robots → [curl]
```

Rust에서는 trait 객체나 `async fn`으로 유사하게 구현할 수 있습니다:

```rust
trait NetworkLayer: Send + Sync {
    async fn process(&self, request: Request) -> Result<Response>;
}
```

---

## 8. 메모리 관리 비교

### 8.1 전략

| 전략 | OxiBrowser | Lightpanda |
|------|-----------|------------|
| **기본 할당자** | Rust 글로벌 할당자 (jemalloc) | GPA (debug) / c_allocator (release) |
| **범위 할당** | `Arc<RwLock<>>` + 일반 할당 | **ArenaPool** (small/medium/large 버킷) |
| **고정 타입 풀** | 없음 | `std.heap.MemoryPool(T)` (CDP, FinalizerCallback) |
| **스코프 관리** | Rust 소유권 + Drop | errdefer 체인 + arena reset |
| **GC 관리** | boa_engine 내부 GC | V8 GC + HandleScope RAII |
| **문자열 관리** | `String` (힙) | Arena 내 할당 + String interning |

### 8.2 Arena 할당 패턴

Lightpanda의 **ArenaPool**은 OxiBrowser가 참고할 만한 패턴입니다:

```zig
// 3개 크기 버킷: small (~4KB), medium (~64KB), large (~256KB)
// 세션/프레임/HTTP 전송마다 arena를 빌리고 반납
pub fn getArena(self: *Session, size_or_bucket: anytype, debug: []const u8) !Allocator
pub fn releaseArena(self: *Session, allocator: Allocator) void

// 사용 예:
const arena = try session.getArena(.medium, "navigate");
defer session.releaseArena(arena);
// arena 내의 모든 할당은 releaseArena 시 한번에 해제
```

Rust에서는 `bumpalo` 크레이트로 유사한 패턴을 구현할 수 있습니다.

### 8.3 DOM-JS 간 메모리 관리

**OxiBrowser:**
- DomSnapshot이 JS 스레드에 `Arc<RwLock<Option<DomSnapshot>>>`으로 전달
- 새 스냅샷 생성 시 이전 것은 drop
- 뮤테이션은 `Arc<RwLock<Vec<DomMutation>>>`으로 수집
- 간단하지만 매 eval마다 전체 DOM을 재직렬화

**Lightpanda:**
- V8 Global 핸들을 Page별 Vec으로 추적
- 임시 핸들은 별도 Map으로 관리하여 빠른 해제
- Call arena: 중첩 콜백에서 depth 추적으로 무효화 방지
- V8 약한 콜백(FinalizerCallback)은 세션 수준 풀에서 할당 (페이지 파괴 후에도 안전)

---

## 9. 동시성 모델 비교

### 9.1 스레딩 모델

```
OxiBrowser (tokio 멀티스레드):
┌─────────────────────────────────────────────────┐
│  tokio runtime (multi-thread)                   │
│  ├── CDP 서버 (tokio TCP + tungstenite)         │
│  ├── 네비게이션 (async reqwest)                 │
│  ├── CDP 이벤트 (mpsc 채널)                     │
│  └── JS 평가 (전용 OS 스레드 + mpsc)           │
│                                                  │
│  동기화:                                         │
│  ├── parking_lot::RwLock (세션 목록)            │
│  ├── tokio::sync::RwLock (개별 세션)            │
│  ├── AtomicBool (closed 플래그)                  │
│  └── std::sync::mpsc (JS 명령/응답)             │
└─────────────────────────────────────────────────┘

Lightpanda (커스텀 이벤트 루프):
┌─────────────────────────────────────────────────┐
│  메인 스레드: Network.run() (poll 기반)         │
│  ├── CDP 연결: 1 OS 스레드 per 연결 (detach)   │
│  ├── HTTP: 단일 스레드 per Client               │
│  ├── fetch 모드: 전용 스레드 (V8 친화성)       │
│  └── MCP 모드: 전용 스레드                     │
│                                                  │
│  동기화:                                         │
│  ├── std.Thread.Mutex (연결 목록, 풀)           │
│  ├── std.atomic.Value(u32) (CAS 연결 카운팅)    │
│  ├── AtomicBool (shutdown 플래그)                │
│  └── RC(T) (.monotonic/.acq_rel 원자적 참조)    │
└─────────────────────────────────────────────────┘
```

### 9.2 핵심 차이점

| 항목 | OxiBrowser | Lightpanda |
|------|-----------|------------|
| **런타임** | tokio (work-stealing 스케줄러) | 커스텀 poll 루프 |
| **비동기 I/O** | tokio async/await | libcurl multi + poll |
| **CDP 연결** | tokio 태스크 | detached OS 스레드 |
| **연결 제한** | `AtomicUsize` (16) | `Atomic(u32)` CAS (설정 가능) |
| **JS 실행** | 전용 스레드 + 채널 | V8 스레드 + Inspector |
| **이벤트 전달** | mpsc 채널 | 동기식 함수 호출 (dispatch 스레드) |

**분석:** tokio 기반의 OxiBrowser가 더 관용적이고 확장성이 좋습니다. 하지만 Lightpanda의 직접적인 제어가 CDP 타이밍 민감성에 유리할 수 있습니다 (예: 이벤트 순서 보장).

---

## 10. 보안 기능 비교

| 기능 | OxiBrowser | Lightpanda |
|------|:----------:|:----------:|
| TLS 인증서 검증 | ✅ (rustls, 설정 가능) | ✅ (BoringSSL, 설정 가능) |
| IP 필터링 (SSRF 방지) | ❌ | ✅ (사설 IP 대역 차단, 커스텀 CIDR) |
| Robots.txt 준수 | ❌ | ✅ (RFC 9309 완전) |
| Web Bot Auth | ❌ | ✅ (Ed25519 서명) |
| 인증서 에러 무시 | ⚠️ (config 플래그만) | ✅ (Security.setIgnoreCertificateErrors) |
| UA 위조 감지 | ❌ | ✅ ("Mozilla" 포함 시 거부) |
| Public Suffix List | ❌ | ✅ (~10,000 엔트리) |
| 크래시 보고 | ❌ | ✅ (fork+curl, 옵트아웃 가능) |

**분석:** SSRF 방지(IP 필터링)와 Robots.txt 준수는 AI 에이전트 브라우저에서 **필수적**인 보안 기능입니다. OxiBrowser에 즉시 추가해야 합니다.

---

## 11. 테스트 및 품질 비교

### 11.1 테스트 인프라

| 항목 | OxiBrowser | Lightpanda |
|------|-----------|------------|
| **단위 테스트** | ~149개 | ~수천개 (파일 내 `test` 블록) |
| **E2E 테스트** | 15개 (순수 Rust) | 전용 테스트 프레임워크 |
| **테스트 HTTP 서버** | `TestHttpServer` (raw TCP) | `TestHTTPServer.zig` |
| **벤치마크** | 5개 (Criterion) | 없음 |
| **퍼징** | 없음 | 없음 |
| **CI** | 미확인 | GitHub Actions |

### 11.2 테스트 커버리지

**OxiBrowser 테스트 커버리지 (양호):**
- DOM 파싱: ✅ (11개 테스트)
- CSS 선택자: ✅
- 인코딩 감지: ✅ (22개 테스트)
- 쿠키 관리: ✅ (13개 테스트)
- JS 런타임: ✅ (~55개 테스트)
- CDP E2E: ✅ (15개 테스트)
- 네비게이션: ✅ (E2E)
- DOM 뮤테이션: ✅

**OxiBrowser 미커버 영역:**
- 동시성 (여러 세션, 여러 CDP 연결)
- 에러 복구 (타임아웃, DNS 실패, 연결 끊김)
- 대용량 HTML (메모리 압력)
- 엣지 케이스 (빈 입력, malformed URL, 매우 깊은 DOM)

---

## 12. 코드 레벨 상세 비교

### 12.1 에러 처리

**OxiBrowser — thiserror + Result:**
```rust
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("navigation failed: {0}")]
    NavigationFailed(String),
    #[error("network error: {0}")]
    NetworkError(String),
    #[error("JS error: {0}")]
    JsError(String),
    // 15개 variant
}
pub type Result<T> = std::result::Result<T, CoreError>;
```

**Lightpanda — Zig 에러 유니온 + errdefer:**
```zig
// 함수는 에러 유니온 반환
pub fn navigate(self: *Session, url: [:0]const u8) !void {
    var page = try self.createPage();        // try로 에러 전파
    errdefer page.deinit();                  // 실패 시 자동 정리
    try page.frame.navigate(url);
    self.commitPage(page);
}
```

**분석:** Rust의 `thiserror`는 더 풍부한 타입 정보를 제공하지만, Zig의 `errdefer`는 부분 초기화 롤백에 더 간결합니다. Rust에서는 `Drop` 트레이트가 유사한 역할을 합니다.

### 12.2 ID 생성

**OxiBrowser — AtomicU64/AtomicU32:**
```rust
static BROWSER_ID: AtomicU64 = AtomicU64::new(0);
impl BrowserId {
    pub fn next() -> Self { BrowserId(BROWSER_ID.fetch_add(1, Ordering::Relaxed)) }
}
```

**Lightpanda — u32 wrapping increment + 포맷된 접두사:**
```zig
// "FID-0000000001", "REQ-0000000001" 등 CDP 호환 포맷
pub fn nextFrameId(self: *Session) u32 {
    self.frame_id_gen +%= 1;  // wrapping add
    return self.frame_id_gen;
}
// Incrementing 제네릭: "SID-1", "BID-1" 등
pub fn Incrementing(comptime T: type, comptime prefix: []const u8) type
```

**분석:** Lightpanda의 접두사 포맷이 CDP 호환성에 유리합니다. Puppeteer/Playwright는 문자열 ID를 예상합니다. OxiBrowser는 현재 정수 ID를 사용하며, CDP 응답에서 문자열로 변환합니다.

### 12.3 HTTP 클라이언트 래핑

**OxiBrowser — reqwest 래퍼 (간결):**
```rust
pub struct HttpClient { client: reqwest::Client }

impl HttpClient {
    pub async fn fetch_text(&self, url: &Url) -> Result<String> {
        let resp = self.client.get(url.as_str()).send().await?;
        let bytes = resp.bytes().await?;
        Ok(decode_html(&bytes, None))
    }
}
```

**Lightpanda — libcurl 직접 제어 (복잡하지만 강력):**
```zig
pub const Client = struct {
    handles: http.Handles,           // curl multi handle
    transfers: AutoHashMapUnmanaged(u32, *Transfer),
    entry_layer: Layer,              // 미들웨어 체인 진입점
    cache_layer: CacheLayer,
    robots_layer: RobotsLayer,
    web_bot_auth_layer: WebBotAuthLayer,
    interception_layer: InterceptionLayer,
    
    pub fn request(self: *Client, req: Request, owner: ?*Owner) !void {
        const transfer = try self.createTransfer(req, owner);
        try self.entry_layer.request(transfer);  // 미들웨어 체인으로
    }
}
```

---

## 13. Lightpanda에서 배울 점

### 🔴 최우선 (CDP 호환성)

1. **Target 도메인 확장** — `sendMessageToTarget`, `createBrowserContext`, `setAutoAttach`는 Puppeteer/Playwright 연동에 필수
2. **Page lifecycle 이벤트** — `frameNavigated`, `domContentEventFired`, `loadEventFired`, `lifecycleEvent(networkIdle)` 필수
3. **Network.getResponseBody** — 콘텐츠 추출에 필수
4. **쿠키 CRUD** — `Network.getCookies/setCookie/deleteCookies/clearBrowserCookies`
5. **Active/Pending 페이지 상태 기계** — HTTP 왕복 중 이전 페이지 유지

### 🟡 중간 우선순위 (기능)

6. **HTTP 미들웨어 체인** — Interception → Auth → Cache → Robots → 실제 요청
7. **IP 필터링** — SSRF 방지 (사설 IP 대역 차단)
8. **Robots.txt 준수** — RFC 9309
9. **DOM.performSearch with XPath** — 많은 도구가 XPath 사용
10. **Input 도메인** — 키보드/마우스 이벤트 (dispatchKeyEvent, dispatchMouseEvent)
11. **Security.setIgnoreCertificateErrors** — 테스트 환경에서 필수
12. **Network.setExtraHTTPHeaders** — 요청 수정
13. **Emulation.setUserAgentOverride** — UA 변경

### 🟢 장기적 개선

14. **Accessibility 도메인** — 접근성 테스트
15. **벤더 도메인 (OXI.*)** — AI 에이전트 전용 기능 (getMarkdown, clickNode 등)
16. **HTTP 캐시** — 반복 페이지 로드 최적화
17. **Arena 할당** — `bumpalo`로 메시지 처리 최적화
18. **이벤트 시스템 개선** — enable/disable 시 실제 register/unregister
19. **Web Worker** — 백그라운드 스크립트 실행
20. **ES 모듈** — `import/export` 지원

---

## 14. OxiBrowser만의 장점

Lightpanda에 없는 OxiBrowser의 강점도 있습니다:

### 14.1 순수 Rust, C 의존성 제로

```
Lightpanda 의존성: V8 (C++), libcurl, BoringSSL, brotli, zlib, nghttp2, libidn2
OxiBrowser 의존성: 모든 것이 Rust (rustls, reqwest, boa_engine, html5ever)
```

- **크로스 컴파일 용이** — C 툴체인 불필요
- **작은 바이너리** — V8 없이 ~5-10MB 예상 (Lightpanda는 50MB+)
- **보안 감사 용이** — C FFI 경계의 메모리 안전 버그 없음
- **빌드 단순** — `cargo build`면 끝

### 14.2 tokio 비동기 런타임

- **관용적 Rust** — async/await, Future, Stream
- **효율적인 I/O 멀티플렉싱** — epoll/kqueue/iocp 자동 선택
- **백프레셔** — async 기반 자연스러운 흐름 제어
- **생태계 호환** — tower, hyper, tonic 등과 통합 용이

### 14.3 견고한 소유권 모델

- Rust의 borrow checker가 컴파일 타임에 데이터 경쟁 방지
- `Arc<RwLock<>>` 패턴은 검증된 동시성 제어
- UAF(Use-After-Free), double-free 불가능
- Lightpanda는 수동 메모리 관리로 인해 GC 경계에서 버그 발생 가능

### 14.4 모듈식 크레이트 구조

- 독립적인 컴파일 단위로 빌드 시간 최적화
- `oxibrowser-webapi`는 브라우저 없이도 DOM 파싱 라이브러리로 단독 사용 가능
- 크레이트별로 다른 의존성 정책 적용 가능

### 14.5 Criterion 벤치마크

- HTML 파싱, DOM 쿼리, Markdown 변환에 대한 체계적인 벤치마크
- Lightpanda에는 공식 벤치마크가 없음

---

## 15. 실행 로드맵 제안

### Phase 1: CDP 호환성 기초 (2-4주)

| 작업 | 난이도 | 영향 |
|------|--------|------|
| `Network.getResponseBody` 구현 | 중간 | Puppeteer 콘텐츠 추출 |
| 쿠키 CRUD (Network + Storage 도메인) | 쉬움 | 세션 관리 |
| `Network.setExtraHTTPHeaders` | 쉬움 | 요청 수정 |
| `Emulation.setUserAgentOverride` | 쉬움 | UA 변경 |
| `Security.setIgnoreCertificateErrors` | 쉬움 | 테스트 환경 |
| Target 도메인 보강 (`sendMessageToTarget`, `createBrowserContext`) | 어려움 | Puppeteer 호환 |
| Page lifecycle 이벤트 보강 | 중간 | 프레임워크 호환 |
| `DOM.performSearch` (XPath 지원) | 어려움 | 요소 탐색 |

### Phase 2: 네트워크 보안 (2-3주)

| 작업 | 난이도 | 영향 |
|------|--------|------|
| IP 필터링 (사설 IP 차단) | 중간 | SSRF 방지 |
| Robots.txt 파서 + 캐시 | 중간 | 크롤링 준수 |
| HTTP 미들웨어 체인 아키텍처 | 어려움 | 확장성 |
| Fetch 도메인 실제 인터셉션 | 어려움 | 요청 수정/합성 |

### Phase 3: WebAPI 확장 (4-8주)

| 작업 | 난이도 | 영향 |
|------|--------|------|
| `fetch()` JS API를 HttpClient에 연결 | 중간 | 웹앱 호환 |
| `setTimeout/setInterval` 비동기 구현 | 어려움 | 타이머 의존 코드 |
| 이벤트 시스템 (addEventListener/dispatchEvent) | 어려움 | 상호작용 |
| Input 도메인 (키보드/마우스) | 중간 | 자동화 |
| `XMLHttpRequest` 구현 | 어려움 | 레거시 호환 |
| WebSocket 클라이언트 (페이지 내) | 어려움 | 실시간 앱 |
| Web Crypto (AES, HMAC) | 중간 | 현대 웹앱 |

### Phase 4: AI 에이전트 특화 (2-4주)

| 작업 | 난이도 | 영향 |
|------|--------|------|
| `OXI.getMarkdown` 벤더 도메인 | 쉬움 | AI 친화적 출력 |
| `OXI.getInteractiveElements` | 중간 | AI 액션 발견 |
| `OXI.getStructuredData` (JSON-LD, OG) | 중간 | 메타데이터 추출 |
| `OXI.clickNode/fillNode` | 중간 | AI 직접 조작 |
| Accessibility 트리 | 어려움 | 접근성 테스트 |

### Phase 5: 성능 및 프로덕션화 (지속적)

| 작업 | 난이도 | 영향 |
|------|--------|------|
| Arena 할당 (bumpalo) 도입 | 중간 | 메모리 효율 |
| HTTP 캐시 (디스크 기반) | 어려움 | 반복 로드 최적화 |
| 다중 세션 동시성 테스트 | 중간 | 안정성 |
| 메모리 프로파일링 | 중간 | 최적화 |
| 크래시 복구 | 어려움 | 안정성 |

---

## 부록 A: 파일 대응표

| 기능 | OxiBrowser 파일 | Lightpanda 파일 |
|------|-----------------|-----------------|
| 엔트리 포인트 | `crates/oxibrowser/src/main.rs` | `src/main.zig` |
| 앱 설정 | `crates/oxibrowser-core/src/config.rs` | `src/Config.zig` |
| 브라우저 | `crates/oxibrowser-core/src/browser.rs` | `src/browser/Browser.zig` (미확인, App.zig가 더 유사) |
| 세션 | `crates/oxibrowser-core/src/session.rs` | `src/browser/Session.zig` |
| 페이지 | `crates/oxibrowser-core/src/page.rs` | (Session에 통합) |
| 프레임 | `crates/oxibrowser-core/src/frame.rs` | `src/browser/Frame.zig` (webapi에) |
| DOM 문서 | `crates/oxibrowser-webapi/src/dom/document.rs` | `src/browser/webapi/Document.zig` |
| DOM 노드 | `crates/oxibrowser-webapi/src/dom/node.rs` | `src/browser/webapi/Node.zig` |
| DOM 트리 | `crates/oxibrowser-webapi/src/dom/tree.rs` | `src/browser/webapi/Node.zig` (통합) |
| JS 런타임 | `crates/oxibrowser-core/src/js/runtime.rs` | `src/browser/js/js.zig` + `Env.zig` + `Context.zig` |
| HTTP 클라이언트 | `crates/oxibrowser-core/src/network/client.rs` | `src/browser/HttpClient.zig` + `src/network/` |
| 쿠키 | `crates/oxibrowser-core/src/network/cookie.rs` | `src/cookies.zig` + `src/browser/storage/Cookie.zig` |
| CDP 서버 | `crates/oxibrowser-cdp/src/server.rs` | `src/Server.zig` |
| CDP 세션 | `crates/oxibrowser-cdp/src/session.rs` | `src/cdp/CDP.zig` |
| CDP 프로토콜 | `crates/oxibrowser-cdp/src/protocol.rs` | `src/cdp/CDP.zig` (통합) |
| CDP 디스패치 | `crates/oxibrowser-cdp/src/domains/mod.rs` | `src/cdp/CDP.zig` (dispatch 함수) |
| Page 도메인 | `crates/oxibrowser-cdp/src/domains/page.rs` | `src/cdp/domains/page.zig` |
| DOM 도메인 | `crates/oxibrowser-cdp/src/domains/dom.rs` | `src/cdp/domains/dom.zig` |
| Runtime 도메인 | `crates/oxibrowser-cdp/src/domains/runtime.rs` | `src/cdp/domains/runtime.zig` |
| Network 도메인 | `crates/oxibrowser-cdp/src/domains/network.rs` | `src/cdp/domains/network.zig` |
| 인코딩 | `crates/oxibrowser-core/src/encoding.rs` | (html5ever의 encoding 모듈) |

## 부록 B: 통계 요약

```
╔══════════════════════════╦════════════╦════════════╗
║        메트릭            ║ OxiBrowser ║ Lightpanda ║
╠══════════════════════════╬════════════╬════════════╣
║ 총 줄 수                 ║   10,623   ║  123,481   ║
║ 소스 파일 수             ║      38    ║      376   ║
║ JS 엔진 바이너리         ║   ~1-2MB   ║  ~30-50MB  ║
║ CDP 도메인               ║       7    ║       20   ║
║ CDP 메서드               ║      ~47   ║     ~100   ║
║ WebAPI 타입              ║      ~15   ║     160+   ║
║ HTML 요소 타입           ║       1    ║       67   ║
║ 이벤트 타입              ║       0    ║       20   ║
║ 네트워크 미들웨어        ║       0    ║        4   ║
║ 단위 테스트              ║     ~149   ║   ~수천    ║
║ E2E 테스트               ║      15    ║   (별도)   ║
║ 벤치마크                 ║       5    ║        0   ║
║ C 의존성                 ║       0    ║        6+  ║
║ 외부 기여자              ║       ?    ║     활발   ║
║ 라이선스                 ║    MIT     ║  AGPL-3.0  ║
╚══════════════════════════╩════════════╩════════════╝
```

---

> **결론:** OxiBrowser는 견고한 아키텍처 토대 위에 구축되어 있지만, 기능 구현 측면에서 Lightpanda에 비해 초기 단계입니다. 가장 시급한 갭은 **Target 도메인 lifecycle**, **Page lifecycle 이벤트**, **Network.getResponseBody**, **쿠키 CRUD**, 그리고 **IP 필터링/Robots.txt** 보안 기능입니다. 장기적으로는 Lightpanda의 미들웨어 체인 패턴과 AI 에이전트 특화 벤더 도메인(Lightpanda의 `LP.*`)을 참고하여 차별화할 수 있습니다. OxiBrowser만의 순수 Rust, 제로 C 의존성, tokio 기반 아키텍처는 경쟁 우위가 될 수 있습니다.

---

*보고서 생성: 5개 병렬 서브에이전트 분석 (총 4,722줄 상세 분석) → 종합 비교 보고서 작성*
