# DOM API 추가 설계서

> **날짜**: 2026-05-16  
> **목표**: Phase 1+2 DOM API 14개를 `create_element_object`에 추가  
> **현재 상태**: 279 테스트 통과, 0 실패

## 1. 문제 분석

### 1.1 현재 아키텍처

```
create_element_object(3247~3805, 558줄)
│
├─ 1. 속성 읽기 (3267~3310)           — tag_upper, id_val, class_val 등
├─ 2. 사전 클로저 생성 (3310~3608)     — get_attribute_fn, click_fn, append_child_fn 등
│   └─ 각각 let xxx_fn = unsafe { NativeFunction::from_closure(...) };
├─ 3. value getter/setter (3610~3644)  — FunctionObjectBuilder 패턴
├─ 4. children/parent (3646~3710)      — children_arr, parent_val
└─ 5. ObjectInitializer 체인 (3712~3805)
    ├─ .property() × 8                 — tagName, textContent, id, className 등
    ├─ .function() × 9                 — getAttribute, click, appendChild 등
    ├─ .function(inline_closure) × 1    — childNodes (유일한 인라인 클로저)
    ├─ .property() × 1                 — __nodeId
    ├─ .accessor() × 1                 — value
    └─ .build()
```

### 1.2 핵심 문제

boa_engine의 `ObjectInitializer`는 **빌더 패턴 체인**입니다:

```rust
ObjectInitializer::new(ctx)
    .property(...)
    .function(fn_var, name, arity)   // ← fn_var는 미리 생성된 NativeFunction
    .build()
```

`.function()`은 두 가지 방식의 첫 번째 인수를 허용합니다:
1. **변수**: `.function(my_fn, js_string!("name"), 1)` ← 권장
2. **인라인 블록**: `.function({ ... }, js_string!("name"), 1)` ← childNodes만 사용

**문제**: 인라인 블록 패턴은 `childNodes` 1개만 작동했습니다. 여러 개를 추가하면:
- 블록 `{ ... }`이 `NativeFunction`을 반환하지만 체인의 `.function()`으로 연결되지 않음
- Rust 파서가 `}`를 체인의 끝으로 해석
- borrow checker 문제: 클로저가 `dom_snapshot_arc`, `mutations` 등을 캡처

### 1.3 해결 전략

**모든 새 함수를 사전에 `let` 변수로 생성 → 체인에서 변수명으로 참조**

기존 코드가 이미 이 패턴을 사용 중:
```rust
let get_attribute_fn = unsafe { NativeFunction::from_closure(...) };  // 사전 생성
...
.function(get_attribute_fn, js_string!("getAttribute"), 1)            // 체인에서 참조
```

새 API도 동일 패턴으로 추가합니다.

## 2. 설계

### 2.1 변경 파일

| 파일 | 변경 내용 |
|------|-----------|
| `runtime.rs` | `create_element_object`에 14개 사전 클로저 + 체인 추가 |
| `dom_snapshot.rs` | 변경 없음 (탐색 메서드 이미 구현됨) |
| `runtime.rs` (create_context) | style/classList 접근자를 위한 보조 함수 추가 |

### 2.2 추가할 14개 API

#### 그룹 A: 트리 탐색 (getter) — 4개

| API | 반환 | DomSnapshot 메서드 |
|-----|------|-------------------|
| `firstChild` | Element \| null | `first_child(node_id)` |
| `lastChild` | Element \| null | `last_child(node_id)` |
| `nextSibling` | Element \| null | `next_sibling(node_id)` |
| `previousSibling` | Element \| null | `previous_sibling(node_id)` |

**패턴**: 모두 동일한 구조
```rust
let snap_x = dom_snapshot_arc.clone();
let nid_x = node.id;
let mut_x = mutations.clone();
let first_child_fn = unsafe {
    NativeFunction::from_closure(move |_this, _args, ctx| {
        let dom = snap_x.read();
        if let Some(ref s) = *dom {
            if let Some(fid) = s.first_child(nid_x) {
                if let Some(c) = s.nodes.get(&fid) {
                    return Ok(create_element_object(s, c, ctx, &mut_x, &snap_x));
                }
            }
        }
        Ok(JsValue::null())
    })
};
```

> **주의**: `.accessor()` (getter/setter)를 사용하면 더 정확하지만,
> `FunctionObjectBuilder` 추가 변환이 필요합니다.
> 단순 `.function()`을 사용하면 `el.firstChild()` 호출이 필요합니다.
> 
> **결정**: `.accessor()`를 사용하여 `el.firstChild` (프로퍼티 접근) 지원.
> `value` 접근자 패턴(3620~3644)과 동일.

#### 그룹 B: 트리 조작 (method) — 5개

| API | 인수 | 로직 |
|-----|------|------|
| `insertBefore(new, ref)` | 2 | 기존 부모에서 제거 → ref 앞에 삽입 |
| `replaceChild(new, old)` | 2 | old 제거 → new를 같은 위치에 삽입 |
| `removeAttribute(name)` | 1 | attributes HashMap에서 제거 |
| `cloneNode(deep)` | 0~1 | 깊은/얕은 복사, 새 ID 할당 |
| `remove()` | 0 | 부모의 children에서 자신 제거 |

#### 그룹 C: 스타일/클래스 — 2개

| API | 반환 | 설명 |
|-----|------|------|
| `style` | CSSStyleDeclaration-like | setProperty, getPropertyValue, removeProperty |
| `classList` | DOMTokenList-like | add, remove, contains, toggle, length |

#### 그룹 D: 포커스/폼 — 3개

| API | 설명 |
|-----|------|
| `focus()` | noop (headless) |
| `blur()` | noop (headless) |
| `submit()` | noop (headless) |

### 2.3 삽입 위치

```
create_element_object()
│
├─ ... 기존 코드 ...
│
├─ 2. 사전 클로저 생성 (기존: 3310~3608)
│   ├─ 기존 클로저들 (get_attribute_fn, click_fn, append_child_fn 등)
│   └─ element_qsa_fn (기존 마지막 클로저)
│
│   ╔════════════════════════════════════════════════╗
│   ║  [NEW] 14개 클로저 사전 생성 (여기에 삽입)      ║
│   ║  위치: element_qsa_fn 이후, value getter 이전   ║
│   ║  라인: ~3609                                   ║
│   ╚════════════════════════════════════════════════╝
│
├─ 3. value getter/setter (기존: 3610~3644)
├─ 4. children/parent (기존: 3646~3710)
└─ 5. ObjectInitializer 체인 (기존: 3712~3805)
    ├─ 기존 .function() 9개
    │
    │   ╔════════════════════════════════════════════════╗
    │   ║  [NEW] 14개 .function()/.accessor() 추가       ║
    │   ║  위치: childNodes 이후, __nodeId 이전          ║
    │   ║  라인: ~3794                                   ║
    │   ╚════════════════════════════════════════════════╝
    │
    ├─ .property(__nodeId)
    ├─ .accessor(value)
    └─ .build()
```

### 2.4 그룹별 상세 구현

#### A. 트리 탐색 — `.accessor()` 사용

`firstChild`/`lastChild`/`nextSibling`/`previousSibling`은 **프로퍼티**이므로
`.function()`이 아닌 `.accessor()`를 사용합니다.

```rust
// ── 사전 클로저 생성 (위치: line ~3609, element_qsa_fn 이후) ──

// firstChild getter
let snap_fc = dom_snapshot_arc.clone();
let nid_fc = node.id;
let mut_fc = mutations.clone();
let first_child_getter = unsafe {
    NativeFunction::from_closure(move |_this, _args, ctx| {
        let dom = snap_fc.read();
        if let Some(ref s) = *dom {
            if let Some(fid) = s.first_child(nid_fc) {
                if let Some(c) = s.nodes.get(&fid) {
                    return Ok(create_element_object(s, c, ctx, &mut_fc, &snap_fc));
                }
            }
        }
        Ok(JsValue::null())
    })
};
let first_child_getter_fn = FunctionObjectBuilder::new(ctx.realm(), first_child_getter)
    .name(js_string!("get firstChild"))
    .build();

// lastChild, nextSibling, previousSibling — 동일 패턴
```

체인에서:
```rust
.accessor(
    js_string!("firstChild"),
    Some(first_child_getter_fn),
    None,  // setter 없음 (읽기 전용)
    Attribute::default(),
)
```

#### B. 트리 조작 — `.function()` 사용

```rust
// insertBefore(newChild, refChild)
let snap_ib = dom_snapshot_arc.clone();
let nid_ib = node.id;
let mut_ib = mutations.clone();
let insert_before_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let new_child = args.first().cloned().unwrap_or(JsValue::undefined());
        let ref_child = args.get(1).cloned().unwrap_or(JsValue::null());

        let new_id = new_child.as_object()
            .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
            .and_then(|v| v.as_number().map(|n| n as u32));

        let ref_id = if ref_child.is_null() || ref_child.is_undefined() {
            None
        } else {
            ref_child.as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32))
        };

        if let Some(nid) = new_id {
            let mut dom = snap_ib.write();
            if let Some(ref mut s) = *dom {
                // 1. 기존 부모에서 제거
                if let Some(old_parent) = s.nodes.get(&nid).and_then(|n| n.parent) {
                    if old_parent != nid_ib {
                        if let Some(p) = s.nodes.get_mut(&old_parent) {
                            p.children.retain(|&c| c != nid);
                        }
                    }
                }
                // 2. ref_id 위치에 삽입 또는 맨 뒤에 append
                let children = s.nodes.get(&nid_ib)
                    .map(|p| p.children.clone())
                    .unwrap_or_default();
                if let Some(rid) = ref_id {
                    if let Some(pos) = children.iter().position(|&c| c == rid) {
                        if let Some(p) = s.nodes.get_mut(&nid_ib) {
                            p.children.retain(|&c| c != nid);
                            p.children.insert(pos, nid);
                        }
                    }
                } else {
                    if let Some(p) = s.nodes.get_mut(&nid_ib) {
                        p.children.retain(|&c| c != nid);
                        p.children.push(nid);
                    }
                }
                // 3. parent 업데이트
                if let Some(c) = s.nodes.get_mut(&nid) {
                    c.parent = Some(nid_ib);
                }
                mut_ib.write().push(DomMutation::AppendChild {
                    parent_id: nid_ib,
                    child_id: nid,
                });
            }
        }
        Ok(new_child)
    })
};
```

#### C. style/classList — **보조 함수 추출**

`style`과 `classList` 접근자는 **중첩 클로저**를 생성합니다.
이를 `create_element_object` 내부에 직접 넣으면 함수가 너무 길어집니다.

**해결**: `create_style_object`와 `create_classlist_object`를 별도 함수로 추출합니다.

```rust
/// Create a CSSStyleDeclaration-like JS object for a node.
fn create_style_object(
    ctx: &mut Context,
    dom_snapshot_arc: &Arc<RwLock<Option<DomSnapshot>>>,
    node_id: u32,
) -> JsValue {
    let sp = dom_snapshot_arc.clone();
    let set_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let prop = args.first().and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let val = args.get(1).and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped()).unwrap_or_default();
            if !prop.is_empty() {
                let mut dom = sp.write();
                if let Some(ref mut s) = *dom {
                    if let Some(n) = s.nodes.get_mut(&node_id) {
                        n.attributes.insert(format!("style:{}", prop), val);
                    }
                }
            }
            Ok(JsValue::undefined())
        })
    };
    // ... get_fn, rm_fn ...

    let obj = boa_engine::object::ObjectInitializer::new(ctx)
        .function(set_fn, js_string!("setProperty"), 2)
        .function(get_fn, js_string!("getPropertyValue"), 1)
        .function(rm_fn, js_string!("removeProperty"), 1)
        .build();
    obj.into()
}

/// Create a DOMTokenList-like JS object for a node.
fn create_classlist_object(
    ctx: &mut Context,
    dom_snapshot_arc: &Arc<RwLock<Option<DomSnapshot>>>,
    node_id: u32,
) -> JsValue { ... }
```

이 함수들은 `create_element_object` 밖(같은 파일 내)에 정의합니다.
`create_element_object`에서는 getter 클로저 내에서 호출합니다.

**그러나** 문제가 있습니다: `create_style_object`는 `ctx: &mut Context`를 필요로 합니다.
boa의 `NativeFunction::from_closure` 클로저는 `Fn` 트레이트이므로 `&mut Context`를 캡처할 수 없습니다.

**대안**: style/classList의 getter 클로저 내부에서 직접 `ObjectInitializer`를 생성합니다.
이미 `value_getter` 패턴에서 `ctx`를 사용하고 있으므로 같은 방식으로 가능합니다.

```rust
// style getter — ctx를 캡처하는 것은 불가능 (NativeFunction은 Fn)
// 대신: style은 .function()으로 구현 (el.style() 호출)
// 또는: build 이후에 define_property로 accessor 등록
```

**결정**: `style`과 `classList`는 **함수**로 구현합니다:
- `el.style.getPropertyValue("color")` → `el.getStyleValue("color")` (간소화)
- 또는 `el.style` 호출 시마다 새 객체 반환 (비효율적이지만 작동)

가장 깔끔한 방법: **style/classList를 .function()으로 등록**하고,
호출 시 CSSStyleDeclaration/DOMTokenList-like 객체를 반환합니다.

```rust
// style() — returns CSSStyleDeclaration-like object
let snap_st = dom_snapshot_arc.clone();
let nid_st = node.id;
let style_fn = unsafe {
    NativeFunction::from_closure(move |_this, _args, ctx| {
        // ctx는 NativeFunction의 세 번째 인수로 전달됨
        let sp = snap_st.clone();
        let set_fn = unsafe {
            NativeFunction::from_closure(move |_this2, args2, _ctx2| { ... })
        };
        let gp = snap_st.clone();
        let get_fn = unsafe {
            NativeFunction::from_closure(move |_this2, args2, _ctx2| { ... })
        };
        let obj = boa_engine::object::ObjectInitializer::new(ctx)
            .function(set_fn, js_string!("setProperty"), 2)
            .function(get_fn, js_string!("getPropertyValue"), 1)
            .build();
        Ok(JsValue::from(obj))
    })
};
```

> **검증**: `ctx`는 클로저의 세 번째 파라미터로 전달되므로 캡처할 필요 없음.
> 내부 클로저도 `unsafe { NativeFunction::from_closure(...) }`로 생성 가능.
> 이 패턴은 이미 `childNodes` 인라인 클로저(3769~3794)에서 사용 중.

### 2.5 구현 순서

```
Step 1: DomSnapshot 탐색 메서드 확인 (이미 구현됨 — 변경 없음)
Step 2: 14개 사전 클로저 생성 (element_qsa_fn 이후에 삽입)
Step 3: 체인에 14개 .function()/.accessor() 추가
Step 4: 빌드 + 테스트
Step 5: 통합 테스트 추가
```

### 2.6 체인 확장 상세

**현재 체인** (3767~3805):
```rust
        .function(element_qs_fn, js_string!("querySelector"), 1)
        .function(element_qsa_fn, js_string!("querySelectorAll"), 1)
        .function(inline_childNodes_closure, js_string!("childNodes"), 0)
        .property(js_string!("__nodeId"), ...)
        .accessor(js_string!("value"), ...)
        .build();
```

**확장 후**:
```rust
        .function(element_qs_fn, js_string!("querySelector"), 1)
        .function(element_qsa_fn, js_string!("querySelectorAll"), 1)
        .function(inline_childNodes_closure, js_string!("childNodes"), 0)    // 기존
        // ─── [NEW] 트리 탐색 (accessor) ───
        .accessor(js_string!("firstChild"), Some(fc_getter_fn), None, attr())
        .accessor(js_string!("lastChild"), Some(lc_getter_fn), None, attr())
        .accessor(js_string!("nextSibling"), Some(ns_getter_fn), None, attr())
        .accessor(js_string!("previousSibling"), Some(ps_getter_fn), None, attr())
        // ─── [NEW] 트리 조작 (function) ───
        .function(insert_before_fn, js_string!("insertBefore"), 2)
        .function(replace_child_fn, js_string!("replaceChild"), 2)
        .function(remove_attr_fn, js_string!("removeAttribute"), 1)
        .function(clone_fn, js_string!("cloneNode"), 1)
        .function(remove_fn, js_string!("remove"), 0)
        // ─── [NEW] 스타일/클래스 (function) ───
        .function(style_fn, js_string!("style"), 0)  // el.style() → CSSStyleDecl
        .function(classlist_fn, js_string!("classList"), 0)  // el.classList() → DOMTokenList
        // ─── [NEW] 포커스/폼 (function) ───
        .function(focus_fn, js_string!("focus"), 0)
        .function(blur_fn, js_string!("blur"), 0)
        .function(submit_fn, js_string!("submit"), 0)
        // ─── 기존 ───
        .property(js_string!("__nodeId"), ...)
        .accessor(js_string!("value"), ...)
        .build();
```

### 2.7 Attribute import

`.accessor()`의 `Attribute` 파라미터를 위해:
```rust
use boa_engine::property::Attribute;
```

이미 파일 상단에 import되어 있을 것으로 예상 (확인 필요).

### 2.8 테스트 계획

각 API에 대한 단위 테스트를 `runtime.rs`의 `#[cfg(test)]` 모듈에 추가합니다:

```rust
#[test]
fn test_element_first_child() {
    let html = r#"<html><body><div><p>first</p><p>second</p></div></body></html>"#;
    // evaluate: document.querySelector("div").firstChild.tagName === "P"
}

#[test]
fn test_element_insert_before() {
    let html = r#"<html><body><ul><li>A</li><li>C</li></ul></body></html>"#;
    // evaluate:
    // var ul = document.querySelector("ul");
    // var c = ul.children[1];
    // var b = document.createElement("li");
    // ul.insertBefore(b, c);
    // ul.children.length === 3
}
```

## 3. 위험 분석

| 위험 | 가능성 | 영향 | 완화 |
|------|--------|------|------|
| `create_element_object` 재귀 호출 스택 오버플로우 | 중 | 높음 | firstChild 등에서 이미 스텁 패턴 사용 (tagName/id만) |
| 클로저 캡처로 인한 Arc Clone 비용 | 낮 | 낮음 | 14개 Arc clone ≈ 14 atomic increment |
| `style` 접근 방식 (함수 vs 프로퍼티) | 중 | 중간 | `.function("style")`은 비표준이지만 headless에서는 충분 |
| `insertBefore` 복잡도 | 중 | 높음 | edge case: ref_child가 this의 자식이 아닌 경우 |

## 4. 대안 검토

### A. `create_element_object` 리팩토링

함수를 558줄에서 분할:
- `create_base_element()` — property/function 체인
- `add_tree_methods()` — 탐색/조작
- `add_style_methods()` — style/classList

**평가**: 장기적으로는 좋지만, 현재 변경 범위에 비해 과도함.
나중에 별도 PR로 진행.

### B. Document-level API로 등록

`document.insertBefore = ...` 형태로 전역 등록 후 element에 위임.

**평가**: 웹 표준과 다름. Element.prototype에 추가하는 것이 맞음.
boa에서는 prototype 조작이 복잡하므로 `create_element_object`에 직접 추가하는 것이 현실적.

### C. JS eval로 보조 함수 등록

```javascript
Element.prototype.insertBefore = function(newChild, refChild) { ... }
```

**평가**: boa에서 `Element.prototype` 조작 가능 여부 불확실.
Rust 단에서 구현하는 것이 확실.

## 5. 결론

**선택한 접근**: 2.3의 삽입 위치에 따라, 14개 `let` 변수를 사전 생성 후 체인에 추가.
이것이 기존 코드 패턴과 일치하고, 빌더 체인을 깨지 않습니다.

구현은 4개 스텝으로 나누어 진행:
1. **Step A**: 그룹 A (탐색 4개) — 가장 단순, `.accessor()` 사용
2. **Step B**: 그룹 B (조작 5개) — 중간 복잡도, `.function()` 사용
3. **Step C**: 그룹 C+D (스타일/포커스 5개) — 중첩 클로저 포함
4. **Step D**: 테스트 작성
