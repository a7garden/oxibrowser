# Session A: Web Platform (P0.1 + P2 전체)

> **브랜치**: `feat/web-platform`
> **기준 커밋**: `b94dd2c` (main)
> **수정 파일**: `runtime.rs`, `session.rs`, `frame.rs`
> **금지 파일**: `crates/oxibrowser-cdp/**`, `Cargo.toml` (root), `benches/`

---

## 컨텍스트

OxiBrowser은 순수 Rust 헤드리스 브라우저. JS 런타임은 `boa_engine 0.20` 사용.
`JsRuntime`은 전용 std::thread에서 실행되며, `Context`는 `!Send`이므로
모든 JS 조작은 `std::sync::mpsc` 채널로 메시지를 보내 처리함.

**boa 0.20 제약사항**:
- `JsObject`는 `!Send` → `Arc<RwLock<JsObject>>` 불가
- `NativeFunction::from_closure`는 `unsafe {}` 필요
- `ObjectInitializer::new(ctx)`가 `&mut ctx`를 빌리므로 동시에 다른 borrow 불가
- `ctx.register_global_property()` 로 글로벌 등록
- `ctx.register_global_callable()` 로 함수 등록
- `.property(name, value, Attribute::all())` 로 속성 추가
- `.function(fn, name, arity)` 로 메서드 추가

**현재 runtime.rs 구조** (4871줄):
```
create_context() — JS 컨텍스트 생성 + 글로벌 등록
  ├── console (line ~825)
  ├── setTimeout/setInterval (line ~870)
  ├── fetch() (line ~932)
  ├── XMLHttpRequest (line ~1300)
  ├── MutationObserver (line ~1427)
  ├── atob/btoa (line ~1474)
  ├── URL/URLSearchParams (line ~1659)
  ├── TextEncoder/TextDecoder (line ~1886)
  ├── register_document_object() (line ~1935)
  ├── register_window_object() (line ~3400)
  │   ├── navigator (line ~3600)
  │   ├── location (line ~3615)
  │   └── window (line ~3700)
  ├── register_storage() (line ~3300)
  └── register_element_object() (line ~2650)

js_thread_loop() (line ~543) — 메인 메시지 루프
  ├── Evaluate { expression, .. } → ctx.eval() + run_jobs()
  ├── SetDomSnapshot → DomSnapshot 교체
  ├── SetPageUrl → window.location 재설정
  └── ...
```

**Session 구조** (`session.rs`):
```
Session {
    history: Vec<Url>,        // 네비게이션 히스토리
    history_index: usize,     // 현재 위치
    go_back() → Result<()>,   // 뒤로가기 (라인 362)
    go_forward() → Result<()>,// 앞으로가기 (라인 389)
    reload() → Result<()>,    // 새로고침 (라인 415)
}
```

**JsRuntime 메시지 구조** (`runtime.rs` line ~130):
```rust
enum JsMessage {
    Evaluate { expression, timeout, result_tx }
    SetDomSnapshot { snapshot }
    SetPageUrl { url }
    SetFetchChannel { tx }
    SetLocalStorageChannel { tx }
    GetGlobal { name, result_tx }
    SetGlobal { name, value }
    DrainMutations { result_tx }
    ConsoleOutput { result_tx }
    ClearConsole
}
```

---

## 작업 항목

### Task 1: P0.1 — Error Recovery (unwrap 제거)

**목표**: Public API 경로에서 panic 발생 가능한 `.unwrap()` 제거

**파일**: `crates/oxibrowser-core/src/js/runtime.rs`, `crates/oxibrowser-core/src/frame.rs`

**frame.rs** (3개 unwrap):
```bash
grep -n "\.unwrap()" crates/oxibrowser-core/src/frame.rs
```

위 3개를 `unwrap_or_default()` 또는 적절한 기본값으로 교체.

**runtime.rs** — 위험 unwrap 식별:
```bash
# 테스트 코드 내부 unwrap은 제외
grep -n "\.unwrap()" crates/oxibrowser-core/src/js/runtime.rs | grep -v "let _ =\|mod tests\|#\[test\]"
```

**치환 규칙**:
- `ctx.global_object().get(...).unwrap()` → `.unwrap_or(JsValue::undefined())`
- `JsValue::from_object(obj).unwrap()` → `.unwrap_or(JsValue::null())`
- `format!(...).unwrap()` → `format!(...).into()` (JsString은 From<&str>)
- boa 빌더 `.build()` 결과 → 이미 `let _ =` 처리됨
- **주의**: `NativeFunction::from_closure` 내부의 `.unwrap()`은 런타임 에러가 JS 예외로 전파되므로
  `.map_err(|e| JsError::from_opaque(js_string!(e.to_string())))` 로 변환

**검증**:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

---

### Task 2: P2.2 — History API

**목표**: `history.pushState()`, `history.back()`, `history.forward()`, `history.go(delta)` 구현

**파일**: `crates/oxibrowser-core/src/js/runtime.rs`, `crates/oxibrowser-core/src/session.rs`

#### Step 1: JsMessage에 HistoryCommand 추가

`runtime.rs`의 `JsMessage` enum에 추가:
```rust
/// JS history 객체에서 Session으로 보내는 명령
HistoryCommand { command: HistoryCommandType },
```

```rust
#[derive(Debug, Clone)]
pub enum HistoryCommandType {
    PushState { url: String },
    ReplaceState { url: String },
    Back,
    Forward,
    Go { delta: i32 },
}
```

#### Step 2: JsRuntime에 history channel 추가

```rust
// JsRuntimeInner (runtime.rs 내부 구조체)
history_tx: std::sync::mpsc::Sender<HistoryCommandType>,
```

`new()` / `with_config()`에서 채널 생성.

#### Step 3: create_context()에 history 객체 등록

`register_window_object()` (line ~3400) 안에, navigator/location과 같은 위치에 추가:

```rust
// history.pushState(state, title, url)
let history_tx_clone = history_tx.clone();
let history_push_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, _ctx| {
        let url = args.get(2)
            .and_then(|v| v.as_string())
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        let _ = history_tx_clone.send(HistoryCommandType::PushState { url });
        Ok(JsValue::undefined())
    })
};

let history_tx_clone = history_tx.clone();
let history_back_fn = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let _ = history_tx_clone.send(HistoryCommandType::Back);
        Ok(JsValue::undefined())
    })
};

let history_tx_clone = history_tx.clone();
let history_forward_fn = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let _ = history_tx_clone.send(HistoryCommandType::Forward);
        Ok(JsValue::undefined())
    })
};

let history_tx_clone = history_tx.clone();
let history_go_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, _ctx| {
        let delta = args.get(0)
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;
        let _ = history_tx_clone.send(HistoryCommandType::Go { delta });
        Ok(JsValue::undefined())
    })
};

let history_len = /* Session에서 전달받은 history 길이 */;
let history_obj = boa_engine::object::ObjectInitializer::new(ctx)
    .property(js_string!("length"), JsValue::from(history_len as f64), Attribute::all())
    .function(history_push_fn, js_string!("pushState"), 3)
    .function(history_back_fn, js_string!("back"), 0)
    .function(history_forward_fn, js_string!("forward"), 0)
    .function(history_go_fn, js_string!("go"), 1)
    .build();
```

`window_final` 오브젝트에 `.property(js_string!("history"), JsValue::from(history_obj), Attribute::all())` 추가.

#### Step 4: Session에서 history command 처리

`session.rs`에 추가:
```rust
/// JS history 명령을 처리합니다.
pub async fn process_history_commands(&mut self) {
    // JsRuntime의 history channel receiver에서 비동기로 수신
    // PushState → self.history 조작 (네비게이션 없이)
    // Back → self.go_back() 재사용
    // Forward → self.go_forward() 재사용
    // Go { delta } → index 계산 후 navigate
}
```

**주의**: `JsRuntime`이 별도 스레드에서 실행되므로,
history 명령은 기존 fetch channel 패턴과 동일하게
`std::sync::mpsc` → Session의 evaluate 완료 후 `try_recv()` 패턴 사용.

가장 간단한 방법: `evaluate_js()` 완료 후 항상 `drain_history_commands()` 호출.

#### 테스트 (runtime.rs `#[cfg(test)]` 블록에 추가):

```rust
#[test]
fn test_history_push_state() {
    let mut rt = JsRuntime::new();
    rt.block_on(rt.set_page_url("http://example.com/page1"));
    let result = rt.block_on(rt.evaluate("history.pushState({}, '', '/page2')"));
    assert!(result.is_ok());
}

#[test]
fn test_history_length() {
    let mut rt = JsRuntime::new();
    rt.block_on(rt.set_page_url("http://example.com/"));
    let result = rt.block_on(rt.evaluate("history.length"));
    // history.length should be >= 1
    assert!(result.is_ok());
}

#[test]
fn test_history_back_forward() {
    let mut rt = JsRuntime::new();
    rt.block_on(rt.set_page_url("http://example.com/a"));
    let _ = rt.block_on(rt.evaluate("history.pushState({}, '', '/b')"));
    // back + forward should not panic
    let result = rt.block_on(rt.evaluate("history.back()"));
    assert!(result.is_ok());
}
```

---

### Task 3: P2.3 — Location API 완성

**목표**: `location.assign()`, `location.replace()`, `location.reload()`, `location.search`, `location.hash`, `location.host`, `location.port` 추가

**파일**: `crates/oxibrowser-core/src/js/runtime.rs`

#### 수정 위치: `register_window_object()` 내 location_obj 생성 (line ~3637)

**현재** (line ~3615):
```rust
let parsed_url = url::Url::parse(&url_owned);
let loc_href = url_owned.clone();
let loc_origin = ...;
let loc_protocol = ...;
let loc_hostname = ...;
let loc_pathname = ...;

let location_obj = boa_engine::object::ObjectInitializer::new(ctx)
    .property("href", ...)
    .property("origin", ...)
    .property("protocol", ...)
    .property("hostname", ...)
    .property("pathname", ...)
    .build();
```

**수정 후**:
```rust
let parsed_url = url::Url::parse(&url_owned).ok();

// 누락된 속성 추가
let loc_search = parsed_url.as_ref()
    .map(|u| u.query().unwrap_or("").to_string())
    .unwrap_or_default();
let loc_hash = parsed_url.as_ref()
    .map(|u| u.fragment().map(|f| format!("#{}", f)).unwrap_or_default())
    .unwrap_or_default();
let loc_host = parsed_url.as_ref()
    .map(|u| {
        let host = u.host_str().unwrap_or("");
        u.port().map(|p| format!("{}:{}", host, p)).unwrap_or(host.to_string())
    })
    .unwrap_or_default();
let loc_port = parsed_url.as_ref()
    .and_then(|u| u.port().map(|p| p.to_string()))
    .unwrap_or_default();

// Navigation channel (history_tx 재사용 또는 별도 채널)
let nav_tx = history_tx.clone();

let location_assign_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, _ctx| {
        if let Some(url) = args.get(0).and_then(|v| v.as_string()) {
            let _ = nav_tx.send(HistoryCommandType::PushState {
                url: url.to_std_string_escaped()
            });
        }
        Ok(JsValue::undefined())
    })
};

// location_replace_fn, location_reload_fn 도 동일 패턴

let location_obj = boa_engine::object::ObjectInitializer::new(ctx)
    .property(js_string!("href"), JsValue::from(js_string!(loc_href.as_str())), Attribute::all())
    .property(js_string!("origin"), JsValue::from(js_string!(loc_origin.as_str())), Attribute::all())
    .property(js_string!("protocol"), JsValue::from(js_string!(loc_protocol.as_str())), Attribute::all())
    .property(js_string!("host"), JsValue::from(js_string!(loc_host.as_str())), Attribute::all())
    .property(js_string!("hostname"), JsValue::from(js_string!(loc_hostname.as_str())), Attribute::all())
    .property(js_string!("port"), JsValue::from(js_string!(loc_port.as_str())), Attribute::all())
    .property(js_string!("pathname"), JsValue::from(js_string!(loc_pathname.as_str())), Attribute::all())
    .property(js_string!("search"), JsValue::from(js_string!(&format!("?{}", loc_search))), Attribute::all())
    .property(js_string!("hash"), JsValue::from(js_string!(loc_hash.as_str())), Attribute::all())
    .function(location_assign_fn, js_string!("assign"), 1)
    .function(location_replace_fn, js_string!("replace"), 1)
    .function(location_reload_fn, js_string!("reload"), 0)
    .build();
```

**search/hash 빈 값 처리**:
- `search`가 빈 문자열이면 `""` (not `"?")`
- `hash`가 빈 문자열이면 `""` (not `"#"`)

#### 테스트:
```rust
#[test]
fn test_location_search_and_hash() {
    let mut rt = JsRuntime::new();
    rt.block_on(rt.set_page_url("http://example.com/path?q=test#section"));
    let result = rt.block_on(rt.evaluate("location.search"));
    // Should contain "?q=test"
    assert!(result.is_ok());
}

#[test]
fn test_location_assign() {
    let mut rt = JsRuntime::new();
    rt.block_on(rt.set_page_url("http://example.com/"));
    let result = rt.block_on(rt.evaluate("location.assign('/page2')"));
    assert!(result.is_ok());
}
```

---

### Task 4: P2.4 — DOM Event System 완성

**목표**: `Event` 생성자, `dispatchEvent`에서 `event.target` 설정, 페이지 로드 시 `DOMContentLoaded`/`load` 자동 발송

**파일**: `crates/oxibrowser-core/src/js/runtime.rs`

#### Step 1: Event 생성자 등록

`create_context()`에 추가 (MutationObserver 등록 이후):

```rust
// new Event(type, options)
let event_ctor = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let event_type = args.get(0)
            .and_then(|v| v.as_string())
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();

        let bubbles = args.get(1)
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(js_string!("bubbles"), ctx).ok())
            .and_then(|v| v.as_boolean())
            .unwrap_or(false);

        let event_obj = boa_engine::object::ObjectInitializer::new(ctx)
            .property(js_string!("type"), JsValue::from(js_string!(event_type.as_str())), Attribute::all())
            .property(js_string!("target"), JsValue::null(), Attribute::all())
            .property(js_string!("currentTarget"), JsValue::null(), Attribute::all())
            .property(js_string!("bubbles"), JsValue::from(bubbles), Attribute::all())
            .property(js_string!("cancelable"), JsValue::from(false), Attribute::all())
            .property(js_string!("defaultPrevented"), JsValue::from(false), Attribute::all())
            .property(js_string!("timeStamp"), JsValue::from(0.0f64), Attribute::all())
            .build();

        Ok(event_obj.into())
    })
};
let _ = ctx.register_global_callable(js_string!("Event"), 1, event_ctor);
```

#### Step 2: dispatchEvent에서 event.target 설정

기존 `dispatchEvent` 핸들러 (element의 `register_element_object()` 내) 수정.
현재 코드는 `__listeners[eventType]`의 각 콜백을 호출하지만,
`event` 인자의 `target` 속성을 설정하지 않음.

수정: 콜백 호출 전 JS 코드로 `event.target = this` 설정:

```rust
// dispatchEvent 내부 (간략화)
// 기존: listener(event)
// 수정: event.target = this_element → listener(event)

// JS에서 실행:
//   const evt = arguments[0];
//   evt.target = this;
//   evt.currentTarget = this;
//   listener.call(this, evt);
```

boa에서는 `this_obj.set(js_string!("target"), event_obj, true, ctx)` 로 설정 가능.

#### Step 3: 페이지 로드 시 DOMContentLoaded / load 이벤트

`js_thread_loop()`의 `SetPageUrl` 핸들러 (또는 `SetDomSnapshot`) 완료 후,
추가 JS 코드 실행:

```rust
// SetPageUrl 또는 SetDomSnapshot 처리 완료 후:
let fire_events_code = r#"
(function() {
    function fireEvent(type) {
        var listeners = document.__listeners && document.__listeners[type];
        if (listeners) {
            var evt = new Event(type);
            evt.target = document;
            evt.currentTarget = document;
            for (var i = 0; i < listeners.length; i++) {
                try { listeners[i](evt); } catch(e) {}
            }
        }
    }
    fireEvent("DOMContentLoaded");
    fireEvent("load");
})();
"#;
let _ = ctx.eval(Source::from_bytes(fire_events_code));
ctx.run_jobs();
```

이 코드는 `document.__listeners["DOMContentLoaded"]` 와 `document.__listeners["load"]` 에
등록된 모든 콜백을 실행함.

#### 테스트:
```rust
#[test]
fn test_event_constructor() {
    let mut rt = JsRuntime::new();
    let result = rt.block_on(rt.evaluate("new Event('click').type"));
    // Should return "click"
    assert!(result.is_ok());
}

#[test]
fn test_dispatch_event_with_target() {
    let mut rt = JsRuntime::new();
    rt.block_on(rt.set_page_url("http://example.com/"));
    // Create element, add listener, dispatch
    let code = r#"
        var div = document.createElement('div');
        var received = false;
        div.addEventListener('click', function(e) {
            received = true;
        });
        div.dispatchEvent(new Event('click'));
        received
    "#;
    let result = rt.block_on(rt.evaluate(code));
    assert!(result.is_ok());
}
```

---

### Task 5: P2.1 — 누락 JS Globals

**목표**: `crypto.randomUUID()`, `queueMicrotask()`, `self` 추가

**파일**: `crates/oxibrowser-core/src/js/runtime.rs`

#### crypto 객체 확장

현재 `crypto.getRandomValues()` 는 글로벌 함수로 등록됨.
이것을 `crypto` 객체의 메서드로 래핑 + `randomUUID()` 추가:

`create_context()`의 crypto 등록 부근에 추가:

```rust
// crypto.randomUUID()
let random_uuid_fn = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let uuid = uuid::Uuid::new_v4().to_string();
        Ok(JsValue::from(js_string!(uuid.as_str())))
    })
};

// crypto 객체 (getRandomValues + randomUUID)
let crypto_obj = boa_engine::object::ObjectInitializer::new(ctx)
    .function(get_random_values_fn, js_string!("getRandomValues"), 1)
    .function(random_uuid_fn, js_string!("randomUUID"), 0)
    .build();

let _ = ctx.register_global_property(js_string!("crypto"), JsValue::from(crypto_obj), Attribute::all());
```

**주의**: 기존 글로벌 `crypto.getRandomValues`를 `crypto` 객체로 이동.
기존 테스트가 `crypto.getRandomValues(typedArr)` 형태로 호출하는지 확인.
기존 코드가 `crypto`를 글로벌로 등록하는지 검색:

```bash
grep -n "crypto" crates/oxibrowser-core/src/js/runtime.rs | head -20
```

만약 기존이 글로벌 함수 `cryptoGetRandomValues` 형태라면,
`crypto` 객체를 새로 만들고 기존 함수를 메서드로 이동.

#### queueMicrotask

```rust
let queue_microtask_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        if let Some(callback) = args.get(0) {
            // Schedule callback as microtask via Promise.resolve().then()
            let cb = callback.clone();
            // Store callback in temp global, eval Promise.resolve().then()
            ctx.register_global_property(
                js_string!("__microtask_cb"),
                cb.clone(),
                Attribute::all(),
            );
            let _ = ctx.eval(Source::from_bytes(
                "Promise.resolve().then(() => { const f = globalThis.__microtask_cb; delete globalThis.__microtask_cb; f(); })"
            ));
        }
        Ok(JsValue::undefined())
    })
};
let _ = ctx.register_global_callable(js_string!("queueMicrotask"), 1, queue_microtask_fn);
```

#### self = window

```rust
// window 등록 직후
let window_val = ctx.global_object().get(js_string!("window"), ctx).unwrap_or(JsValue::undefined());
let _ = ctx.register_global_property(js_string!("self"), window_val, Attribute::all());
```

#### 테스트:
```rust
#[test]
fn test_crypto_random_uuid() {
    let mut rt = JsRuntime::new();
    let result = rt.block_on(rt.evaluate("crypto.randomUUID()"));
    assert!(result.is_ok());
    // Result should be a UUID string (36 chars with hyphens)
}

#[test]
fn test_queue_microtask() {
    let mut rt = JsRuntime::new();
    let code = r#"
        var result = "before";
        queueMicrotask(() => { result = "after"; });
        result
    "#;
    let result = rt.block_on(rt.evaluate(code));
    assert!(result.is_ok());
}

#[test]
fn test_self_equals_window() {
    let mut rt = JsRuntime::new();
    let result = rt.block_on(rt.evaluate("self === window"));
    // Should be true
    assert!(result.is_ok());
}
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
git commit -m "feat(core): <task description>"
```

**최종 목표**: 230+ tests (기존 205 + 신규 25+), 0 clippy warnings

---

## 금지 사항

- `crates/oxibrowser-cdp/` 내 어떤 파일도 수정하지 말 것
- `Cargo.toml` (root) 수정하지 말 것
- `crates/oxibrowser-core/benches/` 수정하지 말 것
- `crates/oxibrowser/` 수정하지 말 것
