# Design: Agent Layout Evaluation APIs

> `getComputedStyle()` + `getBoundingClientRect()` for OxiBrowser
>
> **목표**: AgentOS 에이전트가 "이 요소가 보이는가?", "어디에 있는가?", "어떻게 생겼는가?"를
> 코드에서 판단할 수 있게 만든다.

## 1. 현재 상태와 갭

### 이미 있는 것

| 기능 | 구현 위치 |
|---|---|
| `element.style.setProperty(k,v)` / `.getPropertyValue(k)` | `runtime.rs` style_fn — snapshot에 `style:{prop}` 키로 저장 |
| `element.classList` (add/remove/toggle/contains) | `runtime.rs` classlist_fn |
| DOM 구조 조회 (querySelector, textContent, innerHTML) | `runtime.rs` |
| JS evaluate (Runtime.evaluate) | CDP Runtime domain |
| 텍스트 기반 스크린샷 PNG | `screenshot.rs` — 8×16 비트맵 폰트 |

### 없는 것 (에이전트가 자가 평가에 필요)

1. **`getComputedStyle(el)`** — 요소의 "최종 계산된" 스타일 반환
2. **`el.getBoundingClientRect()`** — 요소의 레이아웃 박스 (x, y, width, height)
3. **`el.offsetParent` / `el.offsetWidth` / `el.offsetHeight`** — 레이아웃 치수 접근자

## 2. 설계 철학

OxiBrowser는 **실제 CSS 레이아웃 엔진이 없다**. flexbox, grid, margin collapsing을 계산하지 않는다.
이건 버그가 아니라 선택이다. 대신 에이전트에게 **의미론적으로 충분한(Semantically Adequate)**
레이아웃 정보를 제공한다.

### "의미론적으로 충분하다"는 것

에이전트가 판단해야 하는 것:
- ✅ "이 버튼이 화면에 보이는가?" (`display:none` / `visibility:hidden`)
- ✅ "이 텍스트의 색상은?" (`color`, `background-color`)
- ✅ "이 요소의 폰트 크기는?" (`font-size`)
- ✅ "이 입력 필드가 비활성화되었는가?" (`disabled`)
- ✅ "이미지에 alt 텍스트가 있는가?"
- ❌ "이 요소가 정확히 픽셀 (347, 212)에 있는가?" ← 실제 레이아웃 필요, 불가능

즉, **정확한 픽셀 위치** 대신 **의미론적 상태**를 제공한다.

## 3. 아키텍처

```
┌─────────────────────────────────────────────────┐
│                   JS Runtime (boa)               │
│                                                  │
│  element.getBoundingClientRect() ──────────┐     │
│  getComputedStyle(element) ───────────────┐│     │
│                                           ││     │
│  ┌────────────────────────────────┐       ││     │
│  │ LayoutEngine (신규 모듈)        │◄──────┘│     │
│  │                                │◄───────┘     │
│  │  1. DomSnapshot 읽기            │              │
│  │  2. 인라인 style 속성 파싱      │              │
│  │  3. 태그 기본 스타일 적용        │              │
│  │  4. 간이 박스 모델 계산         │              │
│  │  5. ComputedStyle / Rect 반환   │              │
│  └────────────────────────────────┘              │
│       │ read                                     │
│       ▼                                          │
│  ┌────────────────────────────────┐              │
│  │ DomSnapshot (Arc<RwLock>)      │              │
│  │  - nodes: HashMap<u32, DomNode>│              │
│  │  - attributes에 style:* 저장    │              │
│  └────────────────────────────────┘              │
└─────────────────────────────────────────────────┘
```

### 3.1 신규 모듈: `crates/oxibrowser-core/src/css/layout.rs`

```rust
//! Simplified layout engine for agent evaluation.
//!
//! NOT a real CSS layout engine. Computes "semantically adequate" layout
//! information from inline styles, tag defaults, and DOM structure.

/// 계산된 스타일. 에이전트가 판단에 필요한 프로퍼티만.
#[derive(Debug, Clone, Serialize)]
pub struct ComputedStyle {
    pub display: String,           // "block" | "inline" | "none" | "flex" | ...
    pub visibility: String,        // "visible" | "hidden" | "collapse"
    pub opacity: f64,              // 0.0 ~ 1.0
    pub color: String,             // "#000000" 형식
    pub background_color: String,  // "transparent" | "#ffffff"
    pub font_size: String,         // "16px"
    pub font_weight: String,       // "normal" | "bold" | "700"
    pub text_align: String,        // "left" | "center" | "right"
    pub overflow: String,          // "visible" | "hidden" | "scroll" | "auto"
    pub position: String,          // "static" | "relative" | "absolute" | "fixed"
    pub width: Option<String>,     // None = auto
    pub height: Option<String>,    // None = auto
    pub margin: String,            // "0px"
    pub padding: String,           // "0px"
    pub border: String,            // "none"
    pub z_index: Option<String>,   // None = auto

    /// 비표준: 에이전트 판단을 위한 편의 플래그
    pub _visible: bool,            // display != "none" && visibility != "hidden" && opacity > 0
    pub _interactive: bool,        // visible && !disabled && tag이 상호작용 가능
}

/// 요소의 레이아웃 사각형. 실제 픽셀이 아닌 "추정치".
#[derive(Debug, Clone, Serialize)]
pub struct LayoutRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

pub struct LayoutEngine;

impl LayoutEngine {
    /// DomSnapshot에서 특정 노드의 computed style 계산
    pub fn compute_style(snapshot: &DomSnapshot, node_id: u32) -> ComputedStyle;

    /// DomSnapshot에서 특정 노드의 bounding rect 추정
    pub fn compute_rect(snapshot: &DomSnapshot, node_id: u32) -> LayoutRect;
}
```

### 3.2 계산 규칙

#### ComputedStyle 계산

```
최종 값 = Tag 기본값 → 인라인 style 덮어쓰기 → 상속(visibility, color, font-size)
```

**Tag 기본 스타일 테이블** (CSS User Agent Stylesheet의 최소 버전):

```rust
fn tag_defaults(tag: &str) -> StyleMap {
    match tag {
        "div" | "section" | "article" | "main" | "header" | "footer" | "nav" | "aside"
            => map!{ display: "block" },
        "span" | "a" | "strong" | "em" | "code" | "b" | "i" | "u" | "small"
            => map!{ display: "inline" },
        "h1" => map!{ display: "block", font_size: "32px", font_weight: "bold" },
        "h2" => map!{ display: "block", font_size: "24px", font_weight: "bold" },
        "h3" => map!{ display: "block", font_size: "18.72px", font_weight: "bold" },
        "p"   => map!{ display: "block", margin: "16px 0" },
        "button" | "input" | "select" | "textarea"
            => map!{ display: "inline-block", _interactive: true },
        "img" => map!{ display: "inline" },
        "li"  => map!{ display: "list-item" },
        "table" => map!{ display: "table" },
        "tr"  => map!{ display: "table-row" },
        "td" | "th" => map!{ display: "table-cell" },
        "head" | "style" | "script" | "meta" | "link" | "noscript"
            => map!{ display: "none" },
        _ => map!{ display: "block" },  // div가 기본값
    }
}
```

**인라인 style 파싱**:

```
style="display:none; color: red; font-size: 14px"
→ 파싱해서 tag_defaults 덮어쓰기
```

현재 DomSnapshot에 `style:{prop}` → `{val}` 형태로 저장됨. 그리고 원본 `style` 속성도
`style` 키에 전체 문자열로 저장됨. 두 소스 모두 활용:

1. 원본 `style` 속성 파싱 → `display`, `color` 등 추출
2. 이미 저장된 `style:{prop}` 키들도 확인 (setProperty로 설정된 것들)

**상속**: 부모에서 자식으로 상속되는 속성:
- `visibility` (기본 "visible")
- `color` (기본 "#000000")
- `font-size` (기본 "16px")
- `font-weight` (기본 "normal")
- `text-align` (명시적 설정 시에만)

상속 계산은 트리를 root에서 leaf까지 순회하면서 누적.

#### LayoutRect 추정

**실제 픽셀 계산이 불가능**하므로, DOM 순서 기반 추정:

```
규칙:
1. display:none → Rect { x:0, y:0, width:0, height:0 }
2. body 자식들은 세로로 쌓임 (block flow 가정)
3. 각 요소의 height 추정:
   - 명시적 height 스타일 → 사용
   - 텍스트 노드 → 1줄 = 16px (font-size 따라)
   - 자식이 있으면 → 자식들의 합 + padding
4. width: 명시적 width 또는 부모 width (기본 1280)
5. inline 요소 → width = 텍스트 길이 × font-size × 0.6
6. position:absolute/fixed → x,y는 명시적 left/top 또는 (0,0)
```

이 추정치는 **정확하지 않다**. 하지만 에이전트에게 다음 판단에는 충분:
- "요소가 페이지에 있는가?" (Rect != zero)
- "요소 A가 B보다 위에 있는가?" (A.y < B.y)
- "요소가 뷰포트 안에 있는가?" (Rect.top < 720)

### 3.3 JS Web API 구현

#### `document.defaultView.getComputedStyle(element)`

`runtime.rs`의 `create_context()`에서 `window` 객체에 추가:

```javascript
// 에이전트가 호출하는 방식
const style = getComputedStyle(document.querySelector('#my-btn'));
style.display;       // "inline-block"
style.visibility;    // "visible"
style.color;         // "#000000"
style._visible;      // true  ← 편의 프로퍼티
style._interactive;  // true  ← 편의 프로퍼티
```

구현: JS 네이티브 함수 → DomSnapshot 읽기 → `LayoutEngine::compute_style()` →
결과를 CSSStyleDeclaration-like JS 객체로 반환.

반환 객체는 프로퍼티 접근(`style.color`)과 `getPropertyValue('color')` 모두 지원.

#### `element.getBoundingClientRect()`

`create_element_object()`에 메서드 추가:

```javascript
const rect = element.getBoundingClientRect();
rect.x;       // 0.0
rect.y;       // 256.0
rect.width;   // 1280.0
rect.height;  // 48.0
rect.top;     // 256.0
rect.right;   // 1280.0
rect.bottom;  // 304.0
rect.left;    // 0.0
```

구현: JS 네이티브 함수 → DomSnapshot 읽기 → `LayoutEngine::compute_rect()` →
DOMRect-like JS 객체 반환.

#### `element.offsetParent` / `offsetWidth` / `offsetHeight`

element의 accessor로 추가:

```javascript
el.offsetWidth;   // 1280.0  (compute_rect().width)
el.offsetHeight;  // 48.0    (compute_rect().height)
el.offsetParent;  // document.body 또는 null
```

## 4. 파일 변경 내역

```
crates/oxibrowser-core/
├── src/css/
│   ├── mod.rs              # mod layout 추가
│   ├── layout.rs           # ★ 신규: LayoutEngine, ComputedStyle, LayoutRect
│   ├── render.rs           # 변경 없음
│   └── screenshot.rs       # 변경 없음
├── src/js/
│   ├── runtime.rs          # 수정: getComputedStyle, getBoundingClientRect, offset* 추가
│   ├── dom_snapshot.rs     # 수정 없음 (style:* 속성은 이미 저장됨)
│   └── ...
└── tests/
    └── layout_test.rs      # ★ 신규: LayoutEngine 단위 테스트

crates/oxibrowser-cdp/
└── src/domains/
    └── oxi.rs              # 수정: OXI.getElementStates 메서드 추가 (선택)
```

## 5. `layout.rs` 상세 의사코드

```rust
use crate::js::dom_snapshot::{DomNode, DomSnapshot};

/// 계산된 스타일
#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: String,
    pub visibility: String,
    pub opacity: f64,
    pub color: String,
    pub background_color: String,
    pub font_size: f64,        // px 단위 float
    pub font_weight: String,
    pub text_align: String,
    pub overflow: String,
    pub position: String,
    pub width: Option<f64>,    // None = auto
    pub height: Option<f64>,   // None = auto
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub padding_top: f64,
    pub padding_bottom: f64,
    pub z_index: Option<i32>,

    // 편의 플래그
    pub visible: bool,
    pub interactive: bool,
}

#[derive(Debug, Clone)]
pub struct LayoutRect {
    pub x: f64, pub y: f64,
    pub width: f64, pub height: f64,
    pub top: f64, pub right: f64,
    pub bottom: f64, pub left: f64,
}

pub struct LayoutEngine;

impl LayoutEngine {
    /// 노드의 computed style 계산
    pub fn compute_style(snapshot: &DomSnapshot, node_id: u32) -> Option<ComputedStyle> {
        let node = snapshot.nodes.get(&node_id)?;

        // 1. 태그 기본값
        let mut style = Self::tag_defaults(&node.tag);

        // 2. 원본 style 속성 파싱
        if let Some(style_str) = node.attributes.get("style") {
            Self::apply_inline_style(&mut style, style_str);
        }

        // 3. setProperty로 설정된 style:* 속성들
        for (key, val) in &node.attributes {
            if let Some(prop) = key.strip_prefix("style:") {
                Self::set_property(&mut style, prop, val);
            }
        }

        // 4. 부모에서 상속 (visibility, color, font-size)
        Self::inherit_from_parent(snapshot, node_id, &mut style);

        // 5. disabled 속성 체크
        let disabled = node.attributes.get("disabled").is_some();

        // 6. 편의 플래그 계산
        style.visible = style.display != "none"
            && style.visibility != "hidden"
            && style.visibility != "collapse"
            && style.opacity > 0.0;

        style.interactive = style.visible
            && !disabled
            && Self::is_interactive_tag(&node.tag);

        Some(style)
    }

    /// 노드의 bounding rect 추정
    pub fn compute_rect(snapshot: &DomSnapshot, node_id: u32) -> LayoutRect {
        let style = Self::compute_style(snapshot, node_id)
            .unwrap_or_default();

        // display:none → zero rect
        if style.display == "none" {
            return LayoutRect::zero();
        }

        let node = snapshot.nodes.get(&node_id);

        // 세로 위치: body에서부터 이 노드까지의 누적 높이 추정
        let (x, y, w, h) = Self::estimate_box(snapshot, node_id);

        LayoutRect {
            x, y, width: w, height: h,
            top: y,
            left: x,
            bottom: y + h,
            right: x + w,
        }
    }

    // ── 내부 메서드 ──

    fn tag_defaults(tag: &str) -> ComputedStyle { ... }
    fn apply_inline_style(style: &mut ComputedStyle, css: &str) { ... }
    fn set_property(style: &mut ComputedStyle, prop: &str, val: &str) { ... }
    fn inherit_from_parent(snap: &DomSnapshot, id: u32, style: &mut ComputedStyle) { ... }
    fn is_interactive_tag(tag: &str) -> bool { ... }
    fn estimate_box(snap: &DomSnapshot, id: u32) -> (f64, f64, f64, f64) { ... }
    fn parse_length(val: &str) -> Option<f64> { ... }
    fn parse_color(val: &str) -> String { ... }
}
```

### `apply_inline_style` — CSS 파서 (최소)

```rust
fn apply_inline_style(style: &mut ComputedStyle, css: &str) {
    // "display:none; color: red; font-size: 14px"
    for decl in css.split(';') {
        let decl = decl.trim();
        if let Some((prop, val)) = decl.split_once(':') {
            Self::set_property(style, prop.trim(), val.trim());
        }
    }
}

fn set_property(style: &mut ComputedStyle, prop: &str, val: &str) {
    match prop {
        "display" => style.display = val.to_lowercase(),
        "visibility" => style.visibility = val.to_lowercase(),
        "opacity" => style.opacity = val.parse().unwrap_or(1.0),
        "color" => style.color = Self::parse_color(val),
        "background-color" | "backgroundColor" => style.background_color = Self::parse_color(val),
        "font-size" | "fontSize" => style.font_size = Self::parse_length(val).unwrap_or(16.0),
        "font-weight" | "fontWeight" => style.font_weight = val.to_lowercase(),
        "text-align" | "textAlign" => style.text_align = val.to_lowercase(),
        "overflow" => style.overflow = val.to_lowercase(),
        "position" => style.position = val.to_lowercase(),
        "width" => style.width = Self::parse_length(val),
        "height" => style.height = Self::parse_length(val),
        "margin-top" | "marginTop" => style.margin_top = Self::parse_length(val).unwrap_or(0.0),
        "margin-bottom" | "marginBottom" => style.margin_bottom = Self::parse_length(val).unwrap_or(0.0),
        "padding-top" | "paddingTop" => style.padding_top = Self::parse_length(val).unwrap_or(0.0),
        "padding-bottom" | "paddingBottom" => style.padding_bottom = Self::parse_length(val).unwrap_or(0.0),
        "z-index" | "zIndex" => style.z_index = val.parse().ok(),
        _ => {} // 무시
    }
}
```

### `estimate_box` — 박스 위치 추정

```rust
fn estimate_box(snapshot: &DomSnapshot, target_id: u32) -> (f64, f64, f64, f64) {
    let viewport_w = 1280.0;
    let default_h = 20.0; // 한 줄 기본 높이

    // body에서 target까지의 경로를 찾아 누적 높이 계산
    let mut y_cursor = 0.0;
    let mut x_cursor = 0.0;

    // body의 자식들을 DFS 순회하면서 y 누적
    if let Some(body_id) = snapshot.body_id {
        Self::accumulate_y(snapshot, body_id, target_id, &mut 0.0, viewport_w);
    }

    // 간이 계산: body의 자식들 중 target이 몇 번째 block인지 카운트
    let y = Self::estimate_y_position(snapshot, target_id);
    let node = snapshot.nodes.get(&target_id);

    // width 추정
    let w = Self::estimate_width(snapshot, target_id, viewport_w);

    // height 추정
    let h = Self::estimate_height(snapshot, target_id);

    (0.0, y, w, h)
}

fn estimate_y_position(snapshot: &DomSnapshot, target_id: u32) -> f64 {
    // body의 자식들을 순회하면서 block 요소의 누적 높이를 y로 사용
    let body_id = match snapshot.body_id {
        Some(id) => id,
        None => return 0.0,
    };

    let mut y = 0.0;
    if let Some(body) = snapshot.nodes.get(&body_id) {
        for &child_id in &body.children {
            if child_id == target_id {
                return y;
            }
            let child_style = Self::compute_style(snapshot, child_id);
            if let Some(s) = &child_style {
                if s.display != "none" {
                    y += Self::estimate_height(snapshot, child_id)
                        + s.margin_top + s.margin_bottom
                        + s.padding_top + s.padding_bottom;
                }
            }
        }

        // 더 깊은 레벨에서 재귀 탐색
        for &child_id in &body.children {
            if let Some(y_found) = Self::find_y_recursive(snapshot, child_id, target_id, y) {
                return y_found;
            }
            let child_style = Self::compute_style(snapshot, child_id);
            if let Some(s) = &child_style {
                if s.display != "none" {
                    y += Self::estimate_height(snapshot, child_id)
                        + s.margin_top + s.margin_bottom
                        + s.padding_top + s.padding_bottom;
                }
            }
        }
    }
    y
}

fn estimate_height(snapshot: &DomSnapshot, node_id: u32) -> f64 {
    let style = Self::compute_style(snapshot, node_id);
    let s = match style {
        Some(s) => s,
        None => return 20.0,
    };

    // 명시적 height
    if let Some(h) = s.height {
        return h;
    }

    // 텍스트 노드 기반 추정
    let node = snapshot.nodes.get(&node_id);
    let text_lines = node.map(|n| {
        let text_len = n.text_content.trim().len() as f64;
        let line_chars = 80.0; // 줄당 대략 80자
        (text_len / line_chars).ceil().max(1.0)
    }).unwrap_or(1.0);

    // 자식 요소가 있으면 자식 높이 합
    let children_height: f64 = node
        .map(|n| {
            n.children.iter()
                .filter_map(|&cid| {
                    let cs = Self::compute_style(snapshot, cid);
                    cs.filter(|s| s.display != "none")
                      .map(|s| Self::estimate_height(snapshot, cid) + s.margin_top + s.margin_bottom)
                })
                .sum()
        })
        .unwrap_or(0.0);

    let text_height = text_lines * s.font_size;

    text_height.max(children_height) + s.padding_top + s.padding_bottom
}

fn estimate_width(snapshot: &DomSnapshot, node_id: u32, parent_w: f64) -> f64 {
    let style = Self::compute_style(snapshot, node_id);
    let s = match style {
        Some(s) => s,
        None => return parent_w,
    };

    if let Some(w) = s.width {
        return w;
    }

    // inline 요소 → 텍스트 길이 기반
    if s.display == "inline" || s.display == "inline-block" {
        let node = snapshot.nodes.get(&node_id);
        return node
            .map(|n| n.text_content.len() as f64 * s.font_size * 0.6)
            .unwrap_or(parent_w);
    }

    // block 요소 → 부모 width
    parent_w
}
```

## 6. JS 바인딩 상세

### 6.1 `getComputedStyle(element)` — window/document에 추가

`create_context()`에서 `window` 객체에 추가:

```rust
// runtime.rs — create_context() 함수 내

// getComputedStyle(element) → CSSStyleDeclaration-like 객체
let gcs_dom = dom_snapshot_arc.clone();
let get_computed_style_fn = unsafe {
    NativeFunction::from_closure(move |_this, args, ctx| {
        let element = args.first().cloned().unwrap_or(JsValue::null());
        let node_id = element
            .as_object()
            .and_then(|o| o.get(js_string!("__nodeId"), ctx).ok())
            .and_then(|v| v.as_number().map(|n| n as u32));

        let node_id = match node_id {
            Some(id) => id,
            None => return Ok(JsValue::null()),
        };

        let dom = gcs_dom.read();
        let snapshot = match dom.as_ref() {
            Some(s) => s,
            None => return Ok(JsValue::null()),
        };

        let computed = LayoutEngine::compute_style(snapshot, node_id);
        let cs = match computed {
            Some(c) => c,
            None => return Ok(JsValue::null()),
        };

        // CSSStyleDeclaration-like 객체 생성
        let obj = ObjectInitializer::new(ctx)
            .property("display", JsValue::from(cs.display.as_str()), Attribute::all())
            .property("visibility", JsValue::from(cs.visibility.as_str()), Attribute::all())
            .property("opacity", JsValue::from(cs.opacity), Attribute::all())
            .property("color", JsValue::from(cs.color.as_str()), Attribute::all())
            .property("backgroundColor", JsValue::from(cs.background_color.as_str()), Attribute::all())
            .property("fontSize", JsValue::from(format!("{}px", cs.font_size)), Attribute::all())
            .property("fontWeight", JsValue::from(cs.font_weight.as_str()), Attribute::all())
            .property("textAlign", JsValue::from(cs.text_align.as_str()), Attribute::all())
            .property("overflow", JsValue::from(cs.overflow.as_str()), Attribute::all())
            .property("position", JsValue::from(cs.position.as_str()), Attribute::all())
            .property("width", cs.width.map(|w| JsValue::from(format!("{}px", w))).unwrap_or(JsValue::from("auto")), Attribute::all())
            .property("height", cs.height.map(|h| JsValue::from(format!("{}px", h))).unwrap_or(JsValue::from("auto")), Attribute::all())
            // 편의 플래그
            .property("_visible", JsValue::from(cs.visible), Attribute::all())
            .property("_interactive", JsValue::from(cs.interactive), Attribute::all())
            // getPropertyValue 메서드도 제공
            .function(get_property_value_fn, js_string!("getPropertyValue"), 1)
            .build();

        Ok(JsValue::from(obj))
    })
};

// window 객체에 추가
.property(
    js_string!("getComputedStyle"),
    JsValue::from(get_computed_style_fn),
    Attribute::all(),
)
```

### 6.2 `element.getBoundingClientRect()` — element에 추가

`create_element_object()`에 추가:

```rust
// getBoundingClientRect() — LayoutEngine 사용
let gbr_dom = dom_snapshot_arc.clone();
let gbr_id = node.id;
let get_bounding_client_rect_fn = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let dom = gbr_dom.read();
        let snapshot = match dom.as_ref() {
            Some(s) => s,
            None => return Ok(JsValue::null()),
        };

        let rect = LayoutEngine::compute_rect(snapshot, gbr_id);

        let obj = boa_engine::object::ObjectInitializer::new(_ctx)
            .property("x", JsValue::from(rect.x), Attribute::all())
            .property("y", JsValue::from(rect.y), Attribute::all())
            .property("width", JsValue::from(rect.width), Attribute::all())
            .property("height", JsValue::from(rect.height), Attribute::all())
            .property("top", JsValue::from(rect.top), Attribute::all())
            .property("right", JsValue::from(rect.right), Attribute::all())
            .property("bottom", JsValue::from(rect.bottom), Attribute::all())
            .property("left", JsValue::from(rect.left), Attribute::all())
            .build();

        Ok(JsValue::from(obj))
    })
};

// element 객체 빌드에 추가
.function(get_bounding_client_rect_fn, js_string!("getBoundingClientRect"), 0)
```

### 6.3 `element.offsetWidth` / `offsetHeight` — element accessor

```rust
// offsetWidth getter
let ow_dom = dom_snapshot_arc.clone();
let ow_id = node.id;
let offset_width_getter = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let dom = ow_dom.read();
        if let Some(ref snap) = *dom {
            let rect = LayoutEngine::compute_rect(snap, ow_id);
            return Ok(JsValue::from(rect.width));
        }
        Ok(JsValue::from(0.0))
    })
};

// offsetHeight getter
let oh_dom = dom_snapshot_arc.clone();
let oh_id = node.id;
let offset_height_getter = unsafe {
    NativeFunction::from_closure(move |_this, _args, _ctx| {
        let dom = oh_dom.read();
        if let Some(ref snap) = *dom {
            let rect = LayoutEngine::compute_rect(snap, oh_id);
            return Ok(JsValue::from(rect.height));
        }
        Ok(JsValue::from(0.0))
    })
};

// element 빌드에 accessor 추가
.accessor("offsetWidth", Some(offset_width_getter), None, Attribute::all())
.accessor("offsetHeight", Some(offset_height_getter), None, Attribute::all())
```

## 7. 테스트 계획

### 7.1 단위 테스트 (`layout_test.rs`)

```rust
#[test]
fn display_none_invisible() {
    let snap = make_snapshot(r#"<div style="display:none">hidden</div>"#);
    let style = LayoutEngine::compute_style(&snap, div_id);
    assert_eq!(style.display, "none");
    assert!(!style.visible);
    let rect = LayoutEngine::compute_rect(&snap, div_id);
    assert_eq!(rect.width, 0.0);
    assert_eq!(rect.height, 0.0);
}

#[test]
fn visibility_hidden_invisible() {
    let snap = make_snapshot(r#"<p style="visibility:hidden">hidden</p>"#);
    let style = LayoutEngine::compute_style(&snap, p_id);
    assert!(!style.visible);
}

#[test]
fn tag_defaults_button_interactive() {
    let snap = make_snapshot(r#"<button>Click</button>"#);
    let style = LayoutEngine::compute_style(&snap, btn_id);
    assert!(style.interactive);
    assert!(style.visible);
}

#[test]
fn disabled_button_not_interactive() {
    let snap = make_snapshot(r#"<button disabled>Click</button>"#);
    let style = LayoutEngine::compute_style(&snap, btn_id);
    assert!(!style.interactive);
}

#[test]
fn head_style_is_display_none() {
    let snap = make_snapshot(r#"<html><head><title>T</title></head><body></body></html>"#);
    let head_style = LayoutEngine::compute_style(&snap, head_id);
    assert_eq!(head_style.display, "none");
}

#[test]
fn inherit_color_from_parent() {
    let snap = make_snapshot(r#"<div style="color:red"><p>child</p></div>"#);
    let child_style = LayoutEngine::compute_style(&snap, p_id);
    assert_eq!(child_style.color, "#ff0000");
}

#[test]
fn inline_element_narrower_than_block() {
    let snap = make_snapshot(r#"<span>short</span><div>text</div>"#);
    let span_rect = LayoutEngine::compute_rect(&snap, span_id);
    let div_rect = LayoutEngine::compute_rect(&snap, div_id);
    assert!(span_rect.width < div_rect.width);
}

#[test]
fn element_below_previous_sibling() {
    let snap = make_snapshot(r#"<div id="a">A</div><div id="b">B</div>"#);
    let a_rect = LayoutEngine::compute_rect(&snap, a_id);
    let b_rect = LayoutEngine::compute_rect(&snap, b_id);
    assert!(b_rect.top > a_rect.top);
}
```

### 7.2 통합 테스트 (JS evaluate로)

```rust
#[tokio::test]
async fn test_js_get_computed_style() {
    let mut session = create_test_session().await;
    session.navigate("data:text/html,<button id='btn' style='display:none'>X</button>").await;

    let result = session.evaluate(
        "JSON.stringify({ display: getComputedStyle(document.getElementById('btn')).display, visible: getComputedStyle(document.getElementById('btn'))._visible })"
    ).await;

    let parsed: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["display"], "none");
    assert_eq!(parsed["visible"], false);
}

#[tokio::test]
async fn test_js_get_bounding_client_rect() {
    let mut session = create_test_session().await;
    session.navigate("data:text/html,<div style='width:200px;height:100px'>Box</div>").await;

    let result = session.evaluate(
        "JSON.stringify(document.querySelector('div').getBoundingClientRect())"
    ).await;

    let parsed: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["width"], 200.0);
    assert_eq!(parsed["height"], 100.0);
}
```

## 8. 구현 순서

```
Phase 1: LayoutEngine 코어 (layout.rs)
  └── ComputedStyle, LayoutRect 구조체
  └── tag_defaults, apply_inline_style, set_property
  └── parse_length, parse_color
  └── compute_style (상속 포함)
  └── 단위 테스트

Phase 2: compute_rect (layout.rs)
  └── estimate_y_position, estimate_height, estimate_width
  └── LayoutRect 계산
  └── 단위 테스트

Phase 3: JS 바인딩 (runtime.rs)
  └── getComputedStyle → window
  └── getBoundingClientRect → element
  └── offsetWidth/offsetHeight → element accessor
  └── 통합 테스트

Phase 4: 검증
  └── AgentOS에서 에이전트 루프로 실제 사용 테스트
```

## 9. 한계와 향후 개선

| 현재 한계 | 개선 방향 |
|---|---|
| Flexbox/Grid 레이아웃 무시 | CSS 속성만 읽고 실제 계산은 안 함 → Phase 5에서 taffy crate 통합 고려 |
| float/absolute 겹침 미반영 | position:absolute/fixed는 좌표만 읽고 실제 배치는 안 함 |
| media query 미지원 | <style> 태그 파싱은 하되 media query는 무시 |
| 색상 파싱 제한적 | hex, rgb(), named colors 지원. hsl(), currentColor 등은 추후 |
| 뷰포트 고정 1280×720 | Page.getFrameMetrics의 실제 값을 반영하도록 개선 |

**이 설계의 의도**: 완벽한 CSS 레이아웃이 아니라, **에이전트가 "보이는가? 상호작용 가능한가? 어디쯤 있는가?"를 판단하는 데 충분한 정보**를 제공하는 것.
