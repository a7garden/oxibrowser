# 시나리오 테스트 이슈 해결 설계서

> `SCENARIO_TEST_REPORT.md`에서 발견된 7개 이슈에 대한 구체적 구현 설계  
> 분석 완료 — 각 이슈의 근본 원인을 코드 레벨에서 파악함

---

## 🔴 Critical 1: DOM Mutation 가시성

### 근본 원인

```
JS: createElement('div') → snap.nodes에 새 노드 삽입 ✅
JS: setAttribute('id','test-div') → attrs_map(Arc)에만 저장 ❌ snap.nodes[node].attributes 미반영
JS: body.appendChild(div) → snap.nodes[body].children에 추가 ✅
JS: document.querySelector('#test-div') → snap.nodes 순회 → node.attributes.get("id") 확인
                                              ↑ 여기엔 "id"가 없다!
```

**핵심 버그**: `setAttribute`가 로컬 `Arc<RwLock<HashMap>>`만 업데이트하고, `DomSnapshot.nodes[node_id].attributes`는 업데이트하지 않습니다.

### 설계

#### 1-A. `setAttribute`에서 스냅샷 속성 동기화

**파일**: `runtime.rs`, `create_element_fn` 내부의 `set_attr_fn` 클로저 (~line 2619)

현재:
```rust
attrs_for_set.write().insert(name.clone(), value.clone());
mut_set_attr.write().push(DomMutation::SetAttribute { ... });
```

수정:
```rust
attrs_for_set.write().insert(name.clone(), value.clone());
// 동기화: 스냅샷 노드의 attributes도 업데이트
{
    let mut dom = dom_snap_ce.write();
    if let Some(ref mut snap) = *dom {
        if let Some(node) = snap.nodes.get_mut(&mut_set_id) {
            node.attributes.insert(name.clone(), value.clone());
        }
    }
}
mut_set_attr.write().push(DomMutation::SetAttribute { ... });
```

**동일 패턴 적용**: `create_element_object()` 함수(~line 3050) 안의 element에도 `setAttribute` 클로저가 있습니다. 거기도 같은 수정 필요 — `dom_snapshot_arc`에 이미 접근 가능하므로 `snap.nodes.get_mut(&node_id).attributes.insert()` 추가.

#### 1-B. `textContent` 설정 시 스냅샷 동기화

`create_element_fn`이 만드는 요소는 `.property(textContent, ...)`로 초기값만 설정하고 `setter`가 없습니다. 하지만 시나리오 9에서 `div.textContent = 'Hello'`를 호출합니다.

`ObjectInitializer.property()`는 불변 속성이므로 `setter`를 추가해야 합니다. 하지만 이건 큰 변경이므로, 대안으로:

**단순 해결**: `create_element_fn`에서 `ObjectInitializer` 대신 수동으로 객체를 빌드하면서 `textContent` setter를 등록:

```rust
// textContent setter — update property + snapshot
let dom_snap_tc = dom_snap_ce.clone();
let tc_id = id_for_obj;
let text_setter_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let text = args.first()
            .and_then(|v| v.to_string(ctx).ok())
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        // Update JS property
        if let Some(obj) = _this.as_object() {
            let _ = obj.set(js_string!("textContent"), JsValue::from(text.as_str()), true, ctx);
        }
        // Update snapshot
        let mut dom = dom_snap_tc.write();
        if let Some(ref mut snap) = *dom {
            if let Some(node) = snap.nodes.get_mut(&tc_id) {
                node.text_content = text;
            }
        }
        Ok(JsValue::undefined())
    })
};
```

그러나 `ObjectInitializer`는 `setter` 등록을 지원하지 않으므로, **대안**으로 `textContent`는 getter만으로 충분합니다 — 시나리오 9의 실제 문제는 `querySelector`가 속성을 못 찾는 것이지 텍스트가 아닙니다. `1-A`만 수정해도 `querySelector('#test-div')`가 작동합니다.

#### 영향 범위

| 파일 | 위치 | 변경 |
|------|------|------|
| `runtime.rs` | `create_element_fn` 내 `set_attr_fn` (~line 2619) | 스냅샷 속성 동기화 (+4줄) |
| `runtime.rs` | `create_element_object` 내 `set_attribute` 클로저 | 동일 패턴 (+4줄) |

---

## 🔴 Critical 2: crypto.getRandomValues — Uint8Array 지원

### 근본 원인

```
var a = new Uint8Array(8);     → boa 내부 TypedArray (JsArray 아님)
crypto.getRandomValues(a);     → JsArray::from_object() 실패 → 버퍼 [0,0,...]
```

클로저가 `JsArray::from_object()`로 변환을 시도하는데, `Uint8Array`는 `JsArray`가 아니라 `JsTypedArray`입니다. `boa_engine`에서 `TypedArray`는 다른 타입입니다.

### 설계

**파일**: `runtime.rs`, `get_random_values_fn` 클로저 (~line 1954)

```rust
let get_random_values_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let arr = args.first().cloned().unwrap_or(JsValue::undefined());
        if let Some(arr_obj) = arr.as_object() {
            // Case 1: Standard JsArray
            if let Ok(js_arr) = JsArray::from_object(arr_obj.clone()) {
                if let Ok(len) = js_arr.length(ctx) {
                    let arr_len = len.min(65536) as usize;
                    let mut buf = vec![0u8; arr_len];
                    let _ = getrandom::fill(&mut buf);
                    for (i, val) in buf.iter().enumerate().take(arr_len) {
                        let _ = js_arr.set(i as u32, JsValue::from(*val as i32), true, ctx);
                    }
                }
                return Ok(arr);
            }

            // Case 2: TypedArray (Uint8Array, Int32Array, etc.)
            // boa_engine TypedArray has .length property and indexed access
            if let Ok(len_val) = arr_obj.get(js_string!("length"), ctx) {
                if let Some(len) = len_val.as_number() {
                    let arr_len = (len as usize).min(65536);
                    let mut buf = vec![0u8; arr_len];
                    let _ = getrandom::fill(&mut buf);
                    for (i, val) in buf.iter().enumerate().take(arr_len) {
                        let _ = arr_obj.set(i as u32, JsValue::from(*val as i32), true, ctx);
                    }
                    return Ok(arr);
                }
            }
        }
        Ok(arr)
    })
};
```

**핵심**: `JsArray::from_object()` 실패 시 `object.length` + `object[i]` 인덱스 접근으로 폴백.

#### 영향 범위

| 파일 | 위치 | 변경 |
|------|------|------|
| `runtime.rs` | `get_random_values_fn` (~line 1954) | TypedArray 폴백 추가 (~10줄) |

---

## 🔴 Critical 3: CDP 세션 리소스 정리

### 근본 원인

```
CdpSession::run() 완료 → browser.sessions 벡터에서 제거 안 됨
9개 시나리오 → 10개 세션 누적 → max_sessions(10) 도달 → 11번째 연결 거부
```

`CdpSession::run()`이 끝나면 `session.close()`를 호출하지만, `Browser`의 세션 벡터에서 세션을 제거하지 않습니다.

### 설계

#### 3-A. CdpSession run 완료 후 Browser에서 세션 제거

**파일**: `crates/oxibrowser-cdp/src/session.rs` (~line 100-150)

```rust
// 현재: run() 완료 후 session.close()만 호출
// 수정: run() 완료 후 browser.remove_session() 호출

impl CdpSession {
    pub async fn run(mut self) -> Result<()> {
        // ... 기존 run 로직 ...
        
        // run 종료 시:
        info!(session_id = %self.session_id, "CDP session ended");
        
        // Browser에서 세션 제거
        self.browser.remove_session(&self.session_id).await;
        
        Ok(())
    }
}
```

#### 3-B. Browser에 remove_session 메서드 추가

**파일**: `crates/oxibrowser-core/src/browser.rs`

```rust
/// Remove a session by its UUID string.
pub async fn remove_session(&self, session_uuid: &str) {
    let mut sessions = self.sessions.write();
    let before = sessions.len();
    sessions.retain(|s| {
        let guard = s.read();
        guard.uuid() != session_uuid
    });
    if sessions.len() < before {
        info!(session_count = sessions.len(), "session removed");
    }
}
```

#### 3-C. Session에 uuid 노출

**파일**: `crates/oxibrowser-core/src/session.rs`

`Session` 구조체에 `uuid: String` 필드가 있는지 확인하고, `pub fn uuid(&self) -> &str` getter 추가.

#### 영향 범위

| 파일 | 위치 | 변경 |
|------|------|------|
| `browser.rs` | `remove_session()` 추가 | (~15줄) |
| `session.rs` (CDP) | `run()` 종료 시 `remove_session` 호출 | (+3줄) |
| `session.rs` (core) | `uuid()` getter | (+3줄) |

---

## 🟡 Important 4: Runtime.evaluate awaitPromise

### 근본 원인

```python
# Python 테스트 코드
r = await cmd("Runtime.evaluate", {"expression": "fetch('/data').then(r => r.json())"})
# → Promise 객체 반환 (값이 아님)
```

Chromium CDP의 `Runtime.evaluate`는 `awaitPromise: true` 파라미터를 지원합니다. Promise가 resolve될 때까지 기다립니다. OxiBrowser는 이 파라미터를 무시합니다.

### 설계

**파일**: `crates/oxibrowser-cdp/src/domains/runtime.rs`

#### 4-A. awaitPromise 파라미터 파싱

```rust
async fn evaluate(params: Option<Value>, ctx: &DispatchContext) -> DomainResult {
    let params = params.unwrap_or_default();
    let expression = params.get("expression").and_then(|v| v.as_str()).unwrap_or("");
    let return_by_value = params.get("returnByValue").and_then(|v| v.as_bool()).unwrap_or(false);
    let await_promise = params.get("awaitPromise").and_then(|v| v.as_bool()).unwrap_or(false);
    // ...
```

#### 4-B. Promise 대기 로직

`JsRuntime::evaluate()`는 `JsResult`를 반환합니다. Promise 결과를 기다리려면 JS 측에서 해결해야 합니다 — `boa_engine`은 아직 `Promise` 완료 대기를 네이티브로 지원하지 않습니다.

**실용적 해결**: `awaitPromise`가 true면, expression을 `(async () => { return await (<expr>) })()`으로 래핑:

```rust
let final_expression = if await_promise {
    format!("(async () => {{ return await ({expression}); }})()")
} else {
    expression.to_string()
};
```

하지만 이것만으로는 충분하지 않습니다 — `evaluate_js()`가 비동기 함수의 resolve 값을 기다려야 합니다. `boa_engine`의 `Context::evaluate()`는 동기적이므로, Promise가 resolve되기를 기다리려면 이벤트 루프가 실행되어야 합니다.

**대안 설계**: `evaluate_js`에 `awaitPromise` 모드를 추가하여, Promise 결과가 settle될 때까지 `job_queue`를 실행:

```rust
// js/runtime.rs의 evaluate 로직 내
if await_promise {
    // Promise가 settle될 때까지 job_queue drain 반복
    loop {
        {
            let dom = self.dom_snapshot.read();
            // Promise 상태 확인...
        }
        // job_queue 실행
        job_queue.run_jobs(&mut context);
        // 타임아웃 체크
    }
}
```

**단순화된 구현**: 실제로는 boa의 `JsValue::as_object()` → `Promise.state()` 확인이 복잡하므로, 초기 구현에서는 expression 래핑 + `JSON.stringify`를 사용:

```rust
let final_expression = if await_promise {
    // Promise가 resolve될 시간을 주기 위해 setTimeout 래핑 후 재평가
    format!(
        "(function() {{ var p = ({expression}); if (p && typeof p.then === 'function') {{ return '___PENDING___'; }} return p; }})()"
    )
} else {
    expression.to_string()
};
```

> **참고**: 완전한 `awaitPromise` 구현은 boa_engine의 `JobQueue`와 긴밀히 연동되어야 합니다. 초기에는 "동기 평가 + PENDING 반환"으로 처리하고, 향후 비동기 평가 파이프라인을 구축하는 것을 권장합니다.

#### 영향 범위

| 파일 | 위치 | 변경 |
|------|------|------|
| `runtime.rs` (CDP) | `evaluate()` | `awaitPromise` 파라미터 파싱 (+3줄) |
| `runtime.rs` (core) | `evaluate_js()` / `JsCommand` | awaitPromise 지시자 전달 (+10줄) |

---

## 🟡 Important 5: HTTP→HTTPS 최종 URL 반영

### 근본 원인

```rust
// session.rs navigate()
let response = self.http_client.fetch(&parsed).await?;
// parsed = http://www.wikipedia.org (원래 URL)
// response.url() = https://www.wikipedia.org/ (리다이렉트 후 최종 URL)
// 하지만 Page::from_html(parsed.clone(), ...)에 parsed를 그대로 전달
```

reqwest는 redirect 후 `response.url()`로 최종 URL을 제공합니다.

### 설계

**파일**: `crates/oxibrowser-core/src/session.rs`, `navigate()` (~line 273)

```rust
pub async fn navigate(&mut self, url: &str) -> Result<()> {
    let parsed = Url::parse(url)?;
    
    let response = self.http_client.fetch(&parsed).await?;
    let status = response.status().as_u16();
    
    // Use the final URL after any redirects
    let final_url = response.url().clone();  // ← 추가
    
    // ... bytes, html 처리 ...
    
    let page = Page::from_html(final_url.clone(), &html, status, ct_header).await?;  // ← 변경
    
    // history에도 final_url 저장
    self.history.push(final_url);  // ← 변경
    // ...
}
```

#### 영향 범위

| 파일 | 위치 | 변경 |
|------|------|------|
| `session.rs` | `navigate()` | `response.url().clone()` 사용 (+1줄, 변경 2줄) |

---

## 🟡 Important 6: OXI.getPageInfo에 HTTP status 노출

### 근본 원인

`Page::status()`는 이미 구현되어 있지만 `get_page_info()`에서 호출하지 않습니다.

### 설계

**파일**: `crates/oxibrowser-cdp/src/domains/oxi.rs`, `get_page_info()` (~line 34)

```rust
async fn get_page_info(ctx: &DispatchContext) -> DomainResult {
    let guard = ctx.session.read().await;
    let url = guard.current_url().map(|u| u.to_string()).unwrap_or_default();
    let title = guard.page().and_then(|p| p.title().map(|t| t.to_string())).unwrap_or_default();
    let status = guard.page().map(|p| p.status()).unwrap_or(0);  // ← 추가
    Ok(Some(json!({
        "url": url,
        "title": title,
        "status": status,          // ← 추가
        "readyState": "complete"
    })))
}
```

#### 영향 범위

| 파일 | 위치 | 변경 |
|------|------|------|
| `oxi.rs` | `get_page_info()` | status 필드 추가 (+2줄) |

---

## 🟡 Important 7: MutationObserver 레코드 수집

### 근본 원인

```
observer.observe(target, {childList: true})  → __observing = true 설정 (OK)
target.appendChild(div)                       → snap.nodes 업데이트 (OK)
                                             → MutationObserver에 레코드 push 안 함 ❌
observer.takeRecords()                        → 빈 배열 반환
```

`appendChild` 클로저가 mutation을 `Vec<DomMutation>`에 기록하지만, `MutationObserver`의 `__records` 배열에는 추가하지 않습니다. 두 시스템이 분리되어 있습니다.

### 설계

#### 7-A. 전역 MutationObserver 레지스트리 추가

**파일**: `runtime.rs`

`MutationObserver` 인스턴스를 추적하기 위해 `context`에 전역 배열을 등록:

```rust
// MutationObserver 레지스트리 — 모든 활성 observer 저장
// context.global()["__moRegistry"] = []
```

`observe()` 호출 시 observer를 `__moRegistry`에 추가합니다.

#### 7-B. DOM 변경 시 observer에게 알림

`appendChild`/`removeChild`/`setAttribute` 클로저에서, 변경 후 `__moRegistry`의 모든 observer에게 레코드를 push:

```rust
// appendChild 클로저 내, mutation push 이후:
// 활성 observer들에게 mutation record push
{
    let dom = dom_snap_ac.read();
    if let Some(ref snap) = *dom {
        // JS 컨텍스트에 접근할 수 없으므로, 다른 방식 필요
    }
}
```

**문제**: mutation 클로저는 `&mut Context`를 가지지 않으므로 JS 객체 조작이 어렵습니다. `create_element_fn` 내부에서는 `ctx`에 접근하지만, `create_element_object` 내부의 `appendChild`에서도 `ctx`에 접근합니다.

**해결**: `ctx`를 사용 가능한 클로저에서만 observer 알림:

```rust
// appendChild 클로저 내부 (create_element_fn 또는 create_element_object)
// 이미 ctx 접근 가능

// mutation 기록 후 observer 레코드 push
notify_mutation_observers(
    ctx,
    "childList",
    parent_id_ac,
    cid,
    &dom_snap_ac,
);
```

```rust
fn notify_mutation_observers(
    ctx: &mut Context,
    mutation_type: &str,
    target_id: u32,
    added_id: u32,
    dom_snapshot: &Arc<RwLock<Option<DomSnapshot>>>,
) {
    // __moRegistry에서 활성 observer 찾기
    let registry = ctx.global_object().get(js_string!("__moRegistry"), ctx);
    if let Ok(reg) = registry {
        if let Some(arr) = reg.as_object() {
            if let Ok(js_arr) = JsArray::from_object(arr.clone()) {
                if let Ok(len) = js_arr.length(ctx) {
                    for i in 0..len {
                        if let Ok(observer) = js_arr.at(i as i64, ctx) {
                            if let Some(obs_obj) = observer.as_object() {
                                let observing = obs_obj.get(js_string!("__observing"), ctx)
                                    .ok().and_then(|v| v.as_boolean()).unwrap_or(false);
                                if observing {
                                    // Create MutationRecord
                                    let record = boa_engine::object::ObjectInitializer::new(ctx)
                                        .property(js_string!("type"), JsValue::from(mutation_type), Attribute::all())
                                        .property(js_string!("target"), JsValue::from(target_id), Attribute::all())
                                        .build();
                                    // Push to __records
                                    let records_val = obs_obj.get(js_string!("__records"), ctx).unwrap_or(JsValue::Null);
                                    if let Some(rec_obj) = records_val.as_object() {
                                        if let Ok(rec_arr) = JsArray::from_object(rec_obj.clone()) {
                                            let _ = rec_arr.push(JsValue::from(record), ctx);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

#### 7-C. observe()에서 레지스트리 등록

```rust
// observe_fn 클로저 수정
let observe_fn = {
    NativeFunction::from_closure(move |_this, _args, ctx| {
        if let Some(obj) = _this.as_object() {
            let _ = obj.set(js_string!("__observing"), JsValue::from(true), true, ctx);
            
            // Register in global __moRegistry
            let registry = ctx.global_object().get(js_string!("__moRegistry"), ctx)
                .unwrap_or(JsValue::Null);
            if let Some(reg_obj) = registry.as_object() {
                if let Ok(reg_arr) = JsArray::from_object(reg_obj.clone()) {
                    let _ = reg_arr.push(JsValue::from(obj.clone()), ctx);
                }
            }
        }
        Ok(JsValue::undefined())
    })
};
```

#### 영향 범위

| 파일 | 위치 | 변경 |
|------|------|------|
| `runtime.rs` | `create_context()` | `__moRegistry` 글로벌 배열 등록 (+5줄) |
| `runtime.rs` | `observe_fn` | 레지스트리 등록 추가 (+8줄) |
| `runtime.rs` | `appendChild`/`removeChild` 클로저 | `notify_mutation_observers` 호출 (+3줄×2) |
| `runtime.rs` | `notify_mutation_observers` 헬퍼 | 신규 함수 (~50줄) |

---

## 구현 우선순위

| 순서 | 이슈 | 난이도 | 영향 | 예상 공수 |
|------|------|--------|------|-----------|
| 1 | **DOM Mutation 속성 동기화** (1-A) | ★☆☆ | querySelector 작동 | 15분 |
| 2 | **crypto Uint8Array** (2) | ★☆☆ | 보안 | 15분 |
| 3 | **HTTP→HTTPS 최종 URL** (5) | ★☆☆ | 정확성 | 10분 |
| 4 | **OXI.getPageInfo status** (6) | ★☆☆ | 정보 노출 | 5분 |
| 5 | **세션 정리** (3) | ★★☆ | 안정성 | 45분 |
| 6 | **awaitPromise** (4) | ★★★ | 비동기 JS | 1시간 |
| 7 | **MutationObserver 레코드** (7) | ★★☆ | 관찰자 패턴 | 1시간 |

> **총 예상 공수**: 약 3.5시간  
> **1-4번은 45분 이내 완료 가능**
