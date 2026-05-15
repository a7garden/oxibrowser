# OxiBrowser 개선 설계서

> 실시간 Hacker News 테스트에서 발견된 5개 제한사항에 대한 구조적 개선 설계

---

## 1. Element-level `querySelector` / `querySelectorAll` 구현

### 현재 상태

```
document.querySelector("a")        → ✅ 작동 (DomSnapshot 전체 트리 DFS)
document.querySelectorAll("a")     → ✅ 작동
element.querySelector("a")         → ❌ undefined
element.querySelectorAll("a")     → ❌ undefined
```

`create_element_object()`가 만드는 JS 요소 객체에는 `querySelector`/`querySelectorAll` 메서드가 등록되지 않습니다. `document`에만 등록되어 있습니다.

### 설계

**핵심 아이디어**: 각 요소에 `data-oxi-node-id`가 이미 부여되어 있으므로, `querySelector` 호출 시 해당 노드를 서브트리 루트로 하는 DFS를 실행합니다.

#### 1-1. DomSnapshot에 서브트리 쿼리 메서드 추가

`crates/oxibrowser-core/src/js/dom_snapshot.rs`:

```rust
impl DomSnapshot {
    /// Query selector scoped to a subtree rooted at `root_id`.
    pub fn query_selector_from(&self, root_id: u32, selector: &str) -> Option<u32> {
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if id != root_id {
                if let Some(node) = self.nodes.get(&id) {
                    if self.node_matches_selector(node, selector) {
                        return Some(id);
                    }
                }
            }
            // Push children
            if let Some(node) = self.nodes.get(&id) {
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        None
    }

    /// Query all matching nodes scoped to a subtree rooted at `root_id`.
    pub fn query_selector_all_from(&self, root_id: u32, selector: &str) -> Vec<u32> {
        let mut results = Vec::new();
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if id != root_id {
                if let Some(node) = self.nodes.get(&id) {
                    if self.node_matches_selector(node, selector) {
                        results.push(id);
                    }
                }
            }
            if let Some(node) = self.nodes.get(&id) {
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        results.reverse();
        results
    }
}
```

#### 1-2. JS 요소 객체에 메서드 등록

`create_element_object()` 내부 (`runtime.rs:~2883`)에 추가:

```rust
// element.querySelector(selector)
let qs_dom = dom_snapshot_arc.clone();
let qs_mutations = mutations.clone();
let qs_root_id = node.id;
let element_qs_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let selector = args.first()
            .and_then(|v| v.as_string())
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();

        let dom = qs_dom.read();
        if let Some(ref snapshot) = *dom {
            if let Some(match_id) = snapshot.query_selector_from(qs_root_id, &selector) {
                if let Some(match_node) = snapshot.nodes.get(&match_id) {
                    return Ok(create_element_object(
                        snapshot, match_node, ctx, &qs_mutations, &qs_dom,
                    ));
                }
            }
        }
        Ok(JsValue::null())
    })
};

// element.querySelectorAll(selector) — 동일 패턴으로 query_selector_all_from 사용
```

그 다음 `ObjectInitializer` 체인에 추가:

```rust
.function(element_qs_fn, js_string!("querySelector"), 1)
.function(element_qsa_fn, js_string!("querySelectorAll"), 1)
```

#### 영향 범위

| 파일 | 변경 |
|------|------|
| `dom_snapshot.rs` | `query_selector_from()` + `query_selector_all_from()` 추가 (~30줄) |
| `runtime.rs` | `create_element_object()`에 2개 클로저 + `.function()` 등록 (~60줄) |

---

## 2. `Array.from()` 폴리필 등록

### 현재 상태

```
Array.from([1,2,3])         → ❌ "not a callable function"
Array.from(nodeList)        → ❌ 동일 에러
```

`boa_engine`은 ES2024+ 명세를 구현하지만 `Array.from`은 아직 누락되어 있습니다 (이슈: boa-dev/boa#XXX). OxiBrowser의 `querySelectorAll`은 이미 `JsArray`를 반환하므로, `Array.from(array)`은 사실상 no-op여야 합니다.

### 설계

`create_context()` 내 `boa_engine` 컨텍스트 초기화 후 폴리필 등록:

```rust
// Array.from() polyfill — boa_engine doesn't provide this yet.
// Register before any user code runs.
let array_from_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let source = args.first().cloned().unwrap_or(JsValue::undefined());

        // Case 1: Already an array → shallow copy
        if let Some(obj) = source.as_object() {
            if let Ok(arr) = JsArray::from_object(obj.clone()) {
                if let Ok(len) = arr.length(ctx) {
                    let items: Vec<JsValue> = (0..len)
                        .filter_map(|i| arr.at(i as i64, ctx).ok())
                        .collect();
                    return Ok(JsArray::from_iter(items, ctx).into());
                }
            }

            // Case 2: Array-like object (has .length + indexed props)
            if let Ok(len_val) = obj.get(js_string!("length"), ctx) {
                if let Some(len) = len_val.as_number() {
                    let items: Vec<JsValue> = (0..len as u32)
                        .filter_map(|i| {
                            obj.get(i, ctx).ok()
                        })
                        .collect();
                    return Ok(JsArray::from_iter(items, ctx).into());
                }
            }
        }

        // Case 3: Single value → wrap in array
        if !source.is_undefined() {
            return Ok(JsArray::from_iter([source], ctx).into());
        }

        Ok(JsArray::new(ctx).into())
    })
};

let _ = context.register_global_callable(js_string!("ArrayFrom"), 1, array_from_fn);

// Then inject a small JS snippet to assign it properly:
let _ = context.eval(Source::from_bytes(
    "if (typeof Array.from === 'undefined') { Array.from = ArrayFrom; }"
));
```

> **참고**: 더 깔끔한 방법은 `context.register_global_property()`로 `Array` 생성자에 `from`을 직접 붙이는 것입니다. 하지만 boa의 `Array` 빌트인 프로토타입 수정이 복잡할 수 있으므로, 글로벌 함수 + JS 스니펫 할당이 안전한 접근입니다.

#### 영향 범위

| 파일 | 변경 |
|------|------|
| `runtime.rs` | `create_context()`에 폴리필 추가 (~40줄) |

---

## 3. CDP Network 이벤트 — `Network.enable` 연동

### 현재 상태

```
CDP 클라이언트가 Network.enable 호출 → network_enabled = true 설정
Page.navigate 핸들러 → send_network_event() → network_enabled 체크 후 전송
```

**문제**: 이벤트는 CDP 세션의 이벤트 브로드캐스트 채널로 전송되지만, WebSocket 응답 루프에서 `id`가 없는 이벤트 메시지를 어떻게 처리하는지가 핵심입니다.

실제 테스트에서 CDP 클라이언트가 `Page.navigate` 후 Network 이벤트를 수신했는지 재확인이 필요합니다.

### 설계

#### 3-1. WebSocket 이벤트 브로드캐스트 검증

`crates/oxibrowser-cdp/src/session.rs`의 이벤트 브로드캐스트:

```
EventSender.send_network_event() → broadcast_tx.send() → 각 WebSocket 세션의 broadcast_rx.recv()
```

현재 `navigate()`에서 emit하는 순서:
1. `Network.requestWillBeSent` (네비게이션 전)
2. (네비게이션 실행)
3. `Page.frameNavigated`
4. `Network.responseReceived`
5. `Network.loadingFinished`
6. `Page.domContentLoadedEventFired`
7. `Page.loadEventFired`

**이벤트는 이미 올바르게 발생하고 있습니다.** 테스트 Python 코드에서 이벤트를 `id` 기반으로 필터링하면서 놓치는 것입니다. 해결책:

#### 3-2. CDP 테스트 클라이언트 수정 (사용자 측)

이벤트 수신을 명시적으로 기다리도록 CDP 테스트 코드 수정:

```python
# Network.enable 후 이벤트 대기
await cmd("Network.enable")  # 추가 필요!
await cmd("Page.navigate", {"url": "https://..."})

# 이벤트 수신 (id 없는 메시지)
while True:
    msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=5))
    if "method" in msg:
        if msg["method"] == "Network.requestWillBeSent":
            print(f"Network event: {msg['method']}")
            break
```

#### 3-3. 서버 측 개선: Network.enable이 없으면 navigate에서 Network 이벤트 스킵

현재 이미 구현되어 있습니다 (`network_enabled` 플래그). **실제 문제는 테스트 클라이언트가 `Network.enable`을 호출하지 않았다는 것입니다.**

#### 추가 제안: Network 도메인에 요청 ID 기반 조회 추가

```rust
// network.rs에 추가
/// Network.getResponseBody — returns body for a completed request.
/// (현재 리소스 본문이 세션에 저장되지 않으므로 TODO)
```

> **장기 개선**: Session에 `HashMap<RequestId, Resource>` 저장소를 추가하여 `Network.getResponseBody` 등 추가 Network CDP 명령을 지원합니다.

#### 영향 범위

| 파일 | 변경 |
|------|------|
| (테스트 클라이언트) | `Network.enable` 호출 추가 |
| `session.rs` | 장기: 요청/응답 추적 저장소 추가 |

---

## 4. Markdown 스마트 헤딩 감지

### 현재 상태

```
<h1>Title</h1>  → "# Title\n\n"  ✅
<span class="titleline">...</span>  → 일반 텍스트  (헤딩 없음)
```

HN은 `<span class="titleline">`을 사용하여 의미상 제목이지만 HTML 헤딩 태그가 아닙니다. 현재 마크다운 변환기는 `<h1>`~`<h6>`만 `#` 헤딩으로 변환합니다.

### 설계

#### 옵션 A: 시맨틱 휴리스틱 (권장)

특정 CSS 클래스/역할을 가진 요소를 헤딩으로 승격:

```rust
// node_to_markdown에서 <span> 분기 내에 추가
"span" => {
    let classes = node.attributes.get("class")
        .map(|s| s.as_str())
        .unwrap_or("");
    let role = node.attributes.get("role")
        .map(|s| s.as_str())
        .unwrap_or("");

    if classes.contains("titleline") || role == "heading" {
        md.push_str("\n### ");  // h3급으로 취급
        self.write_children_text(node_id, md);
        md.push_str("\n\n");
    } else {
        self.write_children_text(node_id, md);
    }
}
```

**문제**: 사이트별 하드코딩은 유지보수성이 나쁩니다.

#### 옵션 B: `aria-level` / `role="heading"` WAI-ARIA 지원 (권장)

```rust
/// Check if an element should be treated as a heading based on ARIA.
fn heading_level(node: &DomNode) -> Option<u8> {
    let role = node.attributes.get("role").map(|s| s.as_str()).unwrap_or("");
    if role == "heading" {
        return node.attributes.get("aria-level")
            .and_then(|v| v.parse().ok())
            .or(Some(2));  // 기본 h2
    }
    None
}
```

#### 옵션 C: AI 에이전트를 위한 구조화된 마크다운 (실용적)

현재 마크다운은 "구조 보존"보다 "내용 추출"에 초점이 맞춰져 있습니다. AI 에이전트에게는 이미 충분합니다. 개선 방향:

```rust
// 새 속성 추가: data-oxi-heading="3"
// 또는 OXI.getStructuredMarkdown — 구조화된 JSON 반환

// OXI domain에 새 명령 추가
"OXI.getStructuredPage" → {
    "title": "...",
    "headings": [{ "level": 1, "text": "..." }, ...],
    "links": [{ "text": "...", "href": "..." }, ...],
    "meta": { "description": "...", "og:image": "..." }
}
```

> **권장**: 옵션 C — OXI 확장 도메인에 구조화된 데이터 API를 추가하는 것이 AI 에이전트 사용 사례에 가장 적합합니다. 마크다운은 "가능한 한 충실하게" 원본 구조를 반영하는 방향으로 개선합니다.

#### 영향 범위

| 파일 | 변경 |
|------|------|
| `document.rs` | `node_to_markdown`에 `role="heading"` + `aria-level` 지원 (~20줄) |
| `oxi.rs` | `OXI.getStructuredPage` 새 CDP 명령 (~60줄) |
| `dom_snapshot.rs` | `headings()`, `links()`, `meta()` 추출 메서드 (~40줄) |

---

## 5. `document.cookie` — HttpOnly 필터링

### 현재 상태

```
서버 응답: Set-Cookie: __cfduid=xxx; HttpOnly; Path=/
JS: document.cookie → "__cfduid=xxx; Path=/"  ← HttpOnly 쿠키가 노출됨
```

`cookies_for_url()`이 `http_only` 플래그를 체크하지 않고 모든 쿠키를 반환합니다. **보안 취약점**입니다.

### 설계

#### 5-1. `CookieJar`에 JS 전용 조회 메서드 추가

```rust
// cookie.rs에 추가

/// Returns cookies visible to JavaScript (excludes HttpOnly cookies).
/// RFC 6265 §5.4: HttpOnly cookies must not be accessible via document.cookie.
pub fn cookies_for_js(&self, url: &Url) -> String {
    let host = url.host_str().unwrap_or("unknown").to_lowercase();
    let url_path = url.path();
    let is_secure = url.scheme() == "https";

    let mut matching: Vec<&CookieEntry> = Vec::new();

    for (domain, entries) in &self.cookies {
        if !domain_matches(&host, domain) && domain != &host {
            continue;
        }
        for cookie in entries {
            if cookie.http_only {
                continue;  // ← 핵심: HttpOnly 쿠키 제외
            }
            if cookie.secure && !is_secure {
                continue;
            }
            let cookie_path = cookie.path.as_deref().unwrap_or("/");
            if !path_matches(url_path, cookie_path) {
                continue;
            }
            matching.push(cookie);
        }
    }

    matching.sort_by(|a, b| {
        let pa = a.path.as_deref().unwrap_or("/").len();
        let pb = b.path.as_deref().unwrap_or("/").len();
        pb.cmp(&pa)
    });

    matching.iter()
        .map(|c| c.to_cookie_header())
        .collect::<Vec<_>>()
        .join("; ")
}
```

#### 5-2. JS cookie getter에서 `cookies_for_js` 사용

```rust
// runtime.rs의 cookie_getter 클로저 수정
let cookie_getter: NativeFunction = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let dom = dom_for_cookie.read();
        if let Some(ref s) = *dom {
            if let Ok(url) = url::Url::parse(&s.url) {
                let guard = cookie_jar_for_get.read();
                if let Some(ref jar) = *guard {
                    // 기존: jar.read().cookies_for_url(&url)
                    // 변경: jar.read().cookies_for_js(&url)  ← HttpOnly 제외
                    let cookies = jar.read().cookies_for_js(&url);
                    return Ok(JsValue::from(JsString::from(cookies.as_str())));
                }
            }
        }
        Ok(JsValue::from(JsString::from("")))
    })
};
```

#### 5-3. HTTP 요청용은 기존 `cookies_for_url` 유지

`HttpClient::fetch()`에서는 HttpOnly 쿠키를 포함하여 전송해야 하므로 `cookies_for_url()`을 그대로 사용합니다.

```
                  ┌─────────────────────────────────┐
                  │          CookieJar               │
                  │  cookies: { domain: [Cookie] }   │
                  ├─────────────────┬───────────────┤
                  │ cookies_for_url │ cookies_for_js │
                  │ (HTTP 요청용)    │ (document.cookie│
                  │ HttpOnly 포함   │ HttpOnly 제외) │
                  └────────┬────────┴───────┬───────┘
                           │                │
                  HttpClient.fetch()   JS getter
```

#### 영향 벜위

| 파일 | 변경 |
|------|------|
| `cookie.rs` | `cookies_for_js()` 메서드 추가 (~30줄) |
| `cookie.rs` | 기존 `cookies_for_url`에 HttpOnly 필터링은 유지하지 않음 (HTTP 전송용) |
| `runtime.rs` | cookie getter를 `cookies_for_js`로 변경 (1줄) |
| `cookie.rs` | 단위 테스트 추가 (~15줄) |

---

## 구현 우선순위

| 순위 | 항목 | 난이도 | 보안 영향 | 예상 공수 |
|------|------|--------|-----------|-----------|
| 🔴 1 | `document.cookie` HttpOnly 필터링 | ★☆☆ | **보안** | 30분 |
| 🟠 2 | Element-level `querySelector` | ★★☆ | 기능 | 1시간 |
| 🟡 3 | `Array.from()` 폴리필 | ★☆☆ | 호환성 | 30분 |
| 🟢 4 | OXI.getStructuredPage | ★★☆ | 편의성 | 2시간 |
| 🔵 5 | Network 이벤트 (이미 작동) | ★☆☆ | — | 테스트만 수정 |

> 총 예상 공수: **약 4시간** (1번~3번은 2시간 이내 완료 가능)
