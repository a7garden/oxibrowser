# OxiBrowser "완전한 Headless" 로드맵

## 현재 상태 (2026-05-16)

```
✅ 가능                    ❌ 불가능
createElement              insertBefore
appendChild                replaceChild  
setAttribute               removeAttribute
getAttribute               cloneNode
querySelector/All          childNodes (live NodeList)
removeChild                firstChild / lastChild / nextSibling
textContent (getter)       textContent (setter on existing)
addEventListener            element.style (CSSStyleDeclaration)
dispatchEvent               classList (DOMTokenList)
MutationObserver (기본)     Event 생성자 / MouseEvent / KeyboardEvent
input.value                 input.checked / input.disabled
                            createDocumentFragment
                            requestAnimationFrame
                            focus() / blur()
                            form.submit()
```

15개 테스트 결과: **2개만 작동, 13개 누락**

---

## Phase 1: React 마운트 최소 요구사항 (2주)

> **목표**: 간단한 React 컴포넌트가 화면에 마운트되는 것

React가 `ReactDOM.render(<App/>, root)`를 수행하는 데 필요한 최소 DOM API:

### 1-1. 트리 조작 API

| API | 중요도 | React에서 하는 일 |
|-----|--------|-------------------|
| `insertBefore(new, ref)` | 🔴 필수 | reconciliation — 기존 노드 앞에 삽입 |
| `replaceChild(new, old)` | 🔴 필수 | DOM 업데이트 |
| `removeAttribute(name)` | 🟡 중요 | 속성 제거 (className→"" 전환 등) |
| `cloneNode(deep)` | 🟡 중요 | 이벤트 위임 최적화 |
| `createDocumentFragment()` | 🟡 중요 | batch DOM 조작 |

### 1-2. 트리 탐색 속성

| 속성 | 중요도 | 설명 |
|------|--------|------|
| `childNodes` | 🔴 필수 | 자식 NodeList (텍스트 노드 포함) |
| `firstChild` | 🔴 필수 | 첫 자식 |
| `lastChild` | 🟡 중요 | 마지막 자식 |
| `nextSibling` | 🔴 필수 | 다음 형제 |
| `previousSibling` | 🟡 중요 | 이전 형제 |
| `nodeValue` | 🟡 중요 | 텍스트 노드 값 |

### 1-3. 스케줄링

| API | 중요도 | 설명 |
|-----|--------|------|
| `requestAnimationFrame(cb)` | 🔴 필수 | React 스케줄러 |
| `cancelAnimationFrame(id)` | 🟡 중요 | 정리 |

### 구현 설계

#### A. DomNode에 탐색 지원 추가

현재 `DomNode` 구조체:
```rust
pub struct DomNode {
    pub id: u32,
    pub tag: String,
    pub attributes: HashMap<String, String>,
    pub text_content: String,
    pub children: Vec<u32>,      // 자식 ID 목록
    pub parent: Option<u32>,     // 부모 ID
    pub node_type: u8,
}
```

탐색 메서드를 `DomSnapshot`에 추가:

```rust
impl DomSnapshot {
    pub fn first_child(&self, node_id: u32) -> Option<u32> {
        self.nodes.get(&node_id)
            .and_then(|n| n.children.first().copied())
    }
    
    pub fn last_child(&self, node_id: u32) -> Option<u32> {
        self.nodes.get(&node_id)
            .and_then(|n| n.children.last().copied())
    }
    
    pub fn next_sibling(&self, node_id: u32) -> Option<u32> {
        let parent_id = self.nodes.get(&node_id).and_then(|n| n.parent)?;
        let parent = self.nodes.get(&parent_id)?;
        let idx = parent.children.iter().position(|&id| id == node_id)?;
        parent.children.get(idx + 1).copied()
    }
    
    pub fn previous_sibling(&self, node_id: u32) -> Option<u32> {
        let parent_id = self.nodes.get(&node_id).and_then(|n| n.parent)?;
        let parent = self.nodes.get(&parent_id)?;
        let idx = parent.children.iter().position(|&id| id == node_id)?;
        if idx > 0 { parent.children.get(idx - 1).copied() } else { None }
    }
}
```

#### B. create_element_object에 속성/메서드 등록

`create_element_object()` 함수에서 각 요소에 다음을 추가:

```rust
// 트리 탐색 속성
.property(js_string!("firstChild"), /* getter closure */, Attribute::default())
.property(js_string!("lastChild"), /* getter closure */, Attribute::default())
.property(js_string!("nextSibling"), /* getter closure */, Attribute::default())
.property(js_string!("previousSibling"), /* getter closure */, Attribute::default())
.property(js_string!("childNodes"), /* NodeList-like object */, Attribute::default())
.property(js_string!("parentNode"), /* already exists via __nodeId lookup */, Attribute::default())

// Mutation methods
.function(insert_before_fn, js_string!("insertBefore"), 2)
.function(replace_child_fn, js_string!("replaceChild"), 2)
.function(remove_attribute_fn, js_string!("removeAttribute"), 1)
.function(clone_node_fn, js_string!("cloneNode"), 1)
.function(remove_fn, js_string!("remove"), 0)
```

#### C. insertBefore 구현 (가장 복잡)

```rust
let insert_before_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let new_child = args.first().cloned().unwrap_or(JsValue::undefined());
        let ref_child = args.get(1).cloned().unwrap_or(JsValue::null());
        
        let new_id = new_child.as_object()
            .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
            .and_then(|v| v.as_number().map(|n| n as u32));
        let ref_id = if ref_child.is_null() {
            None  // null이면 appendChild와 동일
        } else {
            ref_child.as_object()
                .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
                .and_then(|v| v.as_number().map(|n| n as u32))
        };
        
        if let Some(nid) = new_id {
            let mut dom = dom_snap_ib.write();
            if let Some(ref mut snap) = *dom {
                // 1. 기존 부모에서 제거
                if let Some(old_parent) = snap.nodes.get(&nid).and_then(|n| n.parent) {
                    if let Some(p) = snap.nodes.get_mut(&old_parent) {
                        p.children.retain(|&c| c != nid);
                    }
                }
                // 2. 새 부모의 children에서 ref_id 위치 찾기
                if let Some(ref_id) = ref_id {
                    let parent_children = snap.nodes.get(&node_id_ib)
                        .map(|p| p.children.clone()).unwrap_or_default();
                    if let Some(pos) = parent_children.iter().position(|&c| c == ref_id) {
                        if let Some(parent) = snap.nodes.get_mut(&node_id_ib) {
                            parent.children.insert(pos, nid);
                        }
                    }
                } else {
                    // ref_child가 null이면 맨 뒤에
                    if let Some(parent) = snap.nodes.get_mut(&node_id_ib) {
                        if !parent.children.contains(&nid) {
                            parent.children.push(nid);
                        }
                    }
                }
                // 3. 자식의 parent 업데이트
                if let Some(child) = snap.nodes.get_mut(&nid) {
                    child.parent = Some(node_id_ib);
                }
            }
        }
        
        Ok(new_child)
    })
};
```

#### D. childNodes — NodeList 구현

`childNodes`는 getter로 구현. 매번 접근 시 새 NodeList 객체를 생성:

```rust
let dom_snap_cn = dom_snapshot_arc.clone();
let node_id_cn = node.id;
let child_nodes_getter = unsafe {
    NativeFunction::from_closure(move |_this, _args, ctx| {
        let dom = dom_snap_cn.read();
        if let Some(ref snap) = *dom {
            if let Some(node) = snap.nodes.get(&node_id_cn) {
                let items: Vec<JsValue> = node.children.iter()
                    .filter_map(|&cid| snap.nodes.get(&cid))
                    .map(|child| create_element_object(snap, child, ctx, &mutations_cn, &dom_snap_cn))
                    .collect();
                let arr = JsArray::from_iter(items, ctx);
                // length property
                let obj = boa_engine::object::ObjectInitializer::new(ctx)
                    .property(js_string!("length"), JsValue::from(items.len() as i32), Attribute::all())
                    .build();
                return Ok(JsValue::from(obj));
            }
        }
        Ok(JsValue::null())
    })
};
```

#### E. requestAnimationFrame

```rust
let raf_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let callback = args.first().cloned().unwrap_or(JsValue::undefined());
        let raf_id = NEXT_RAF_ID.fetch_add(1, Ordering::Relaxed);
        
        // Timer 방식과 동일 — TokioJobQueue에 등록
        // 즉시 실행 (headless이므로 다음 프레임 대기 불필요)
        if let Some(cb) = callback.as_object() {
            let _ = cb.call(&JsValue::Undefined, &[JsValue::from(raf_id as f64)], ctx);
        }
        
        Ok(JsValue::from(raf_id as f64))
    })
};
```

#### F. Event 생성자

```rust
let event_ctor = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let event_type = args.first()
            .and_then(|v| v.to_string(ctx).ok())
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        
        let prevent_default_fn = {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                // Mark as prevented (simplified)
                Ok(JsValue::undefined())
            })
        };
        let stop_propagation_fn = {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                Ok(JsValue::undefined())
            })
        };
        
        let obj = boa_engine::object::ObjectInitializer::new(ctx)
            .property(js_string!("type"), JsValue::from(event_type.as_str()), Attribute::all())
            .property(js_string!("bubbles"), JsValue::from(false), Attribute::all())
            .property(js_string!("cancelable"), JsValue::from(false), Attribute::all())
            .property(js_string!("target"), JsValue::null(), Attribute::all())
            .property(js_string!("currentTarget"), JsValue::null(), Attribute::all())
            .property(js_string!("defaultPrevented"), JsValue::from(false), Attribute::all())
            .function(prevent_default_fn, js_string!("preventDefault"), 0)
            .function(stop_propagation_fn, js_string!("stopPropagation"), 0)
            .build();
        Ok(JsValue::from(obj))
    })
};
let _ = context.register_global_callable(js_string!("Event"), 1, event_ctor);

// MouseEvent, KeyboardEvent도 동일 패턴
```

### 예상 공수

| 항목 | 공수 |
|------|------|
| DomSnapshot 탐색 메서드 | 2시간 |
| insertBefore + replaceChild | 4시간 |
| childNodes + tree traversal props | 3시간 |
| removeAttribute + cloneNode + remove | 2시간 |
| createDocumentFragment | 1시간 |
| requestAnimationFrame | 1시간 |
| Event/MouseEvent/KeyboardEvent | 3시간 |
| 단위 테스트 | 3시간 |
| **총계** | **~2주 (19시간)** |

---

## Phase 2: 폼 조작 (1주)

> **목표**: "이 폼 채워줘" 시나리오 완수

### 2-1. input.value getter/setter

```rust
// create_element_object에서 tag가 input/textarea/select일 때
if tag_lower == "input" || tag_lower == "textarea" {
    let dom_snap_val = dom_snapshot_arc.clone();
    let val_id = node.id;
    let value_getter = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            // attributes["value"] 우선, 없으면 ""
            let dom = dom_snap_val.read();
            if let Some(ref snap) = *dom {
                if let Some(n) = snap.nodes.get(&val_id) {
                    return Ok(JsValue::from(JsString::from(
                        n.attributes.get("value").map(|s| s.as_str()).unwrap_or("")
                    )));
                }
            }
            Ok(JsValue::from(JsString::from("")))
        })
    };
    let value_setter = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let val = args.first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            // Update snapshot attribute
            let mut dom = dom_snap_val.write();
            if let Some(ref mut snap) = *dom {
                if let Some(n) = snap.nodes.get_mut(&val_id) {
                    n.attributes.insert("value".to_string(), val);
                }
            }
            Ok(JsValue::undefined())
        })
    };
    // .property with getter/setter — boa에서는 define_property 필요
}
```

### 2-2. input.checked, select.value

동일 패턴으로 attributes에서 읽기/쓰기.

### 2-3. element.style (CSSStyleDeclaration)

```rust
// style getter — 빈 CSSStyleDeclaration-like 객체 반환
let style_getter = unsafe {
    NativeFunction::from_closure(move |_this, _args, ctx| {
        // Simplified: just an object with setProperty/getPropertyValue
        let set_prop_fn = unsafe {
            NativeFunction::from_closure(move |_this2, args2, ctx2| {
                // Store in __styleProps
                Ok(JsValue::undefined())
            })
        };
        let obj = boa_engine::object::ObjectInitializer::new(ctx)
            .function(set_prop_fn, js_string!("setProperty"), 2)
            .build();
        Ok(JsValue::from(obj))
    })
};
```

### 2-4. classList (DOMTokenList)

```rust
let class_list_getter = unsafe {
    NativeFunction::from_closure(move |_this, _args, ctx| {
        let add_fn = /* closure that reads "class" attr, adds, writes back */;
        let remove_fn = /* same pattern */;
        let contains_fn = /* check */;
        let toggle_fn = /* add or remove */;
        let obj = boa_engine::object::ObjectInitializer::new(ctx)
            .function(add_fn, js_string!("add"), 1)
            .function(remove_fn, js_string!("remove"), 1)
            .function(contains_fn, js_string!("contains"), 1)
            .function(toggle_fn, js_string!("toggle"), 1)
            .build();
        Ok(JsValue::from(obj))
    })
};
```

### 2-5. focus() / blur()

```rust
let focus_fn = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        // In headless mode, just update document.activeElement
        Ok(JsValue::undefined())
    })
};
```

### 예상 공수

| 항목 | 공수 |
|------|------|
| input.value/checked setter | 3시간 |
| element.style | 4시간 |
| classList | 3시간 |
| focus/blur + activeElement | 2시간 |
| form.submit | 2시간 |
| 테스트 | 4시간 |
| **총계** | **~1주 (18시간)** |

---

## Phase 3: React/Vue 실제 동작 (1주)

> **목표**: CDN React 앱이 마운트되고 상호작용

### 3-1. React 호환성 체크리스트

```
Phase 1 + 2 완료 후 검증:
1. React 18 CDN 로드 (via script tag) — html5ever가 파싱
2. React.createElement → 실제 DOM 마운트
3. useState → 리렌더링 (insertBefore/replaceChild)
4. 이벤트 핸들링 (onClick → dispatchEvent)
5. useEffect (setTimeout + requestAnimationFrame)
```

### 3-2. 추가 필요 Web API

| API | 용도 |
|-----|------|
| `Object.defineProperty` | React의 속성 감지 (boa 기본 지원 확인) |
| `Proxy` | Vue 3 반응형 (boa 기본 지원 확인) |
| `WeakRef` / `FinalizationRegistry` | React GC 최적화 |
| `structuredClone` | 상태 복사 |
| `AbortController` | fetch 취소 |

### 3-3. textContent setter

현재 `create_element_fn`에서 `textContent`는 `.property()`로 설정하므로 불변입니다.
setter를 등록하여 가변으로 만들어야 합니다.

boa_engine의 `ObjectInitializer`는 getter/setter 등록을 직접 지원하지 않으므로,
객체 생성 후 `define_property`로 accessor를 등록:

```rust
let obj = boa_engine::object::ObjectInitializer::new(ctx)
    // ... 기존 초기화 ...
    .build();

// textContent를 getter/setter로 재정의
let dom_snap_tc = dom_snapshot_arc.clone();
let tc_id = node.id;
let tc_getter = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let dom = dom_snap_tc.read();
        if let Some(ref snap) = *dom {
            if let Some(n) = snap.nodes.get(&tc_id) {
                return Ok(JsValue::from(JsString::from(n.text_content.as_str())));
            }
        }
        Ok(JsValue::from(JsString::from("")))
    })
};
let tc_setter = unsafe {
    NativeFunction::from_closure(move |_this, args, _ctx| {
        let text = args.first().and_then(|v| v.as_string())
            .map(|s| s.to_std_string_escaped()).unwrap_or_default();
        let mut dom = dom_snap_tc.write();
        if let Some(ref mut snap) = *dom {
            if let Some(n) = snap.nodes.get_mut(&tc_id) {
                n.text_content = text;
            }
        }
        Ok(JsValue::undefined())
    })
};
```

### 예상 공수

| 항목 | 공수 |
|------|------|
| textContent setter | 3시간 |
| React CDN 로드 테스트 | 4시간 |
| 이벤트 위임 (bubbling) | 6시간 |
| Vue 3 Proxy 호환 | 4시간 |
| 디버깅/수정 | 8시간 |
| **총계** | **~1주 (25시간)** |

---

## 총 로드맵

```
현재 (v0.6.0)
│  ✅ 정적 콘텐츠 스크래핑
│  ✅ 마크다운/AI 추출
│  ✅ 기본 DOM 조작
│  ✅ 쿠키/세션
│  ✅ CDP 10도메인
│
├─ Phase 1 (2주) ── "React 마운트"
│  insertBefore, replaceChild, childNodes,
│  tree traversal, rAF, Event 생성자
│  
├─ Phase 2 (1주) ── "폼 채우기"
│  input.value, style, classList,
│  focus/blur, form.submit
│  
├─ Phase 3 (1주) ── "실제 SPA 동작"
│  textContent setter, 이벤트 버블링,
│  React 18 CDN 마운트 검증
│  
├─ Phase 4 (2-3개월) ── "완전한 headless"
│  CSS 레이아웃 엔진 (servo)
│  Canvas 2D, WebGL
│  iframe, WebSocket
│  Web Workers
│  
└─ Phase 5 (장기) ── "브라우저 대체"
   Service Workers, PWA
   오디오/비디오
   PDF 생성
```

### 즉시 효과 있는 작업 (Phase 1만으로)

Phase 1 완료 시 가능해지는 것:
- ✅ React/Vue 간단한 컴포넌트 마운트
- ✅ SSR 페이지에서 JS hydrate (부분)
- ✅ DOM 조작이 필요한 스크래핑 (대부분의 사이트)

### Phase 1+2 완료 시

- ✅ "이 폼 채워줘" 자동화
- ✅ 로그인 자동화
- ✅ 검색어 입력 + 결과 수집
- ✅ 챗봇 UI 자동 조작
