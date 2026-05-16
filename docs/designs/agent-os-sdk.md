# oxibrowser SDK for Agent OS

> oxibrowser를 에이전트 OS(oxios)에 탑재하기 위한 SDK 설계.
> oxios 관점에서 바라본 API 요구사항과 구조 개선안.

## 1. 현재의 문제

### 1.1 OS가 매번 lock 관리를 해야 함

`Session`의 대부분의 메서드가 `&mut self`를 요구한다. OS는 `Arc<RwLock<Session>>`으로 감싸고, 매 호출마다 lock을 획득해야 한다.

```rust
// OS가 매번 해야 하는 패턴
let session_arc = self.ensure_session().await?;     // lock 1: session 찾기
let mut session = session_arc.write().await;        // lock 2: RwLock 획득
session.navigate(url).await?;
```

읽기(`page()`, `html()`)도 실제로는 `&self`인데, `Arc<RwLock>` 구조상 write lock을 잡아야 하는 경우가 발생한다.

### 1.2 에이전트의 90% 사용 케이스가 3단계다

```
"이 URL 읽어줘" → session 생성 → navigate → page.markdown
```

이게 한 번의 호출이어야 한다.

### 1.3 `BrowserConfig`가 `Deserialize`를 지원하지 않음

OS가 TOML 설정에서 직접 파싱할 수 없어서, 중간 구조체를 만들어 필드를 하나씩 옮겨야 한다.

### 1.4 `click`/`type`이 Session 메서드가 아님

`js/input.rs`에 생성기가 있지만, 소비자가 직접 JS를 조립해서 `evaluate_js`로 실행해야 한다. OS가 30줄짜리 JS를 하드코딩하고 있다.

### 1.5 결과가 흩어져 있음

`navigate()`는 `Result<()>`를 반환한다. 타이틀은 `page().title()`, URL은 `page().url()`, 내용은 `page().to_markdown()`으로 따로 가져와야 한다. "이 페이지에 뭐가 있어?"에 대한 답이 한 객체에 있어야 한다.

---

## 2. 제안 API

### 2.1 `BrowserConfig` — `Deserialize`/`Serialize` 지원

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrowserConfig {
    // 기존 필드 그대로...
}
```

OS의 TOML에서 `[browser.engine]`으로 직접 임베드 가능.

### 2.2 `BrowseResult` — 페이지 탐색의 통합 결과

```rust
/// 에이전트가 "이 페이지에 뭐가 있어?"에 대한 답.
/// 모든 탐색(navigate, back, forward, reload, post)이 반환하는 단일 객체.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseResult {
    /// 최종 URL (리다이렉트 후).
    pub url: String,
    /// 페이지 제목.
    pub title: String,
    /// HTTP 상태 코드.
    pub status: u16,
    /// 페이지 콘텐츠를 Markdown으로 렌더링.
    /// 에이전트의 1순위 콘텐츠.
    pub markdown: String,
    /// 원본 HTML (필요시).
    pub html: String,
}
```

### 2.3 `Browser` — 새 메서드

```rust
impl Browser {
    /// 원샷: URL → 콘텐츠.
    /// 에이전트의 가장 흔한 요청: "이 URL 읽어줘".
    ///
    /// 내부에서 임시 session을 생성 → navigate → BrowseResult 추출 → session 정리.
    /// 쿠키는 browser의 공유 cookie_jar를 통해 세션 간 유지된다.
    pub async fn browse(&self, url: &str) -> Result<BrowseResult>;

    /// 인터랙티브 탭 열기.
    /// 에이전트가 클릭/타이핑/JS 실행이 필요할 때 사용.
    /// Tab은 Clone 가능하며 &self만으로 동작한다.
    pub async fn new_tab(&self) -> Result<Tab>;
}
```

### 2.4 `Tab` — 에이전트 친화적 인터랙티브 세션

```rust
/// 클론 가능하고 `&self`만으로 동작하는 에이전트용 탭.
///
/// 내부에 `Arc<Mutex<Session>>`을 숨겨서 소비자가 lock 관리를 안 해도 됨.
/// `Browser::new_tab()`으로 생성.
pub struct Tab {
    inner: Arc<Mutex<Session>>,
}

impl Tab {
    // --- 탐색 ---
    // 모든 탐색 메서드는 BrowseResult를 반환한다.

    /// URL로 이동.
    pub async fn goto(&self, url: &str) -> Result<BrowseResult>;

    /// 히스토리 뒤로.
    pub async fn back(&self) -> Result<BrowseResult>;

    /// 히스토리 앞으로.
    pub async fn forward(&self) -> Result<BrowseResult>;

    /// 현재 페이지 새로고침.
    pub async fn reload(&self) -> Result<BrowseResult>;

    /// POST 요청 후 결과 페이지 로드.
    pub async fn post(&self, url: &str, body: &str, content_type: &str) -> Result<BrowseResult>;

    // --- 인터랙션 ---
    // js/input.rs의 생성기를 내부에서 사용.
    // 소비자는 JS 코드를 직접 작성하지 않는다.

    /// CSS selector로 요소 클릭.
    pub async fn click(&self, selector: &str) -> Result<()>;

    /// CSS selector로 요소 찾아서 텍스트 입력.
    pub async fn r#type(&self, selector: &str, text: &str) -> Result<()>;

    /// 키 입력 이벤트 발생.
    pub async fn press_key(&self, key: &str) -> Result<()>;

    // --- 콘텐츠 추출 ---

    /// 현재 페이지의 통합 결과.
    pub async fn content(&self) -> Result<BrowseResult>;

    /// CSS selector에 매칭되는 모든 요소의 텍스트.
    pub async fn query_all(&self, selector: &str) -> Result<Vec<String>>;

    /// JavaScript 실행. Promise는 기다리지 않음.
    pub async fn evaluate(&self, js: &str) -> Result<Value>;

    /// JavaScript 실행. Promise 해결을 기다림.
    pub async fn evaluate_await(&self, js: &str) -> Result<Value>;

    // --- 대기 ---

    /// CSS selector가 매칭될 때까지 폴링.
    pub async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<()>;

    // --- 서브 리소스 ---

    /// 페이지에 참조된 JS/CSS/이미지 로드.
    pub async fn load_resources(&self) -> Result<usize>;

    // --- 스크린샷 ---

    /// 텍스트 기반 PNG 스크린샷.
    pub async fn screenshot(&self, width: u32) -> Result<Vec<u8>>;

    // --- 라이프사이클 ---

    /// 탭 닫기.
    pub async fn close(&self) -> Result<()>;

    /// 탭이 닫혔는지 확인.
    pub fn is_closed(&self) -> bool;
}
```

#### 설계 결정

| 결정 | 이유 |
|------|------|
| `&self` 전용 | OS가 `Arc<RwLock<Tab>>`이 아니라 `Arc<Tab>`으로 공유 가능 |
| `BrowseResult` 통합 반환 | 매번 `page()` → `title()` → `to_markdown()` 체인 불필요 |
| `click`/`type` 내장 | `js/input.rs` 생성기 + `evaluate_js`를 내부에서 조합 |
| `Tab`은 `Clone` | 여러 에이전트가 같은 탭 참조 가능 |
| `Tab` 내부 lock은 `Mutex` | 탭은 단일 에이전트가 순차 사용, `RwLock` 오버헤드 불필요 |

---

## 3. OS 통합이 어떻게 변하는가

### Before (현재)

```
┌─ oxios config.rs ─────┐
│  BrowserConfig (5필드) │  ← 1단계: TOML 파싱
└───────────┬────────────┘
            ↓ 변환
┌─ OxibrowserConfig ────┐
│  (동일 5필드 중복)      │  ← 2단계: 불필요한 중간 구조체
└───────────┬────────────┘
            ↓ 변환
┌─ BrowserConfig ───────┐
│  (실제 15+ 필드)       │  ← 3단계: 대부분 기본값으로 버려짐
└────────────────────────┘

┌─ BrowserBackend trait ─┐  ← Session API 1:1 복제 (17개 메서드)
└────────┬────────────────┘
         ↓ 구현
┌─ OxibrowserBackend ────┐  ← lock 획득 + 1줄 위임 × 17
└────────┬────────────────┘
         ↓ 사용
┌─ BrowserTool ──────────┐  ← action match × 17
└────────┬────────────────┘
         ↓ 파사드
┌─ browser_api.rs ───────┐  ← BrowserApi 래퍼
└────────────────────────┘

파일 4개, oxibrowser API 추가 시 4군데 수정
```

### After (제안)

```rust
// browser_tool.rs — 단일 파일

pub struct BrowserTool {
    browser: Arc<oxibrowser_core::Browser>,
    tab: Arc<Mutex<Option<oxibrowser_core::Tab>>>,
}

impl AgentTool for BrowserTool {
    async fn execute(&self, ...) {
        match action {
            "browse" => {
                // 원샷: URL → Markdown. 세션 관리 불필요.
                let result = self.browser.browse(url).await?;
                Ok(AgentToolResult::success(result.markdown))
            }
            "goto" | "click" | "type" | ... => {
                // 인터랙티브: Tab 하나 유지하면서 작업.
                let tab = self.get_or_create_tab().await?;
                let result = tab.goto(url).await?;
                Ok(AgentToolResult::success(result.markdown))
            }
        }
    }
}
```

```
파일 1개, oxibrowser API 추가 시 1군데만 수정
```

---

## 4. 마이그레이션 가이드

### Phase 1: 기존 API 유지 + 새 API 추가

- `BrowserConfig`에 `Deserialize`/`Serialize` derive 추가
- `BrowseResult` 구조체 추가
- `Browser::browse()`, `Browser::new_tab()` 추가
- `Tab` 구조체 추가 (기존 `Session` 래핑)
- 기존 `Session` API는 그대로 유지 (CDP 등 다른 소비자용)

### Phase 2: OS 통합 단순화

OS 쪽에서:
- `BrowserBackend` trait 제거
- `OxibrowserConfig` 제거
- `OxibrowserBackend` 제거
- `browser_api.rs` 제거
- `BrowserTool` 단일 파일로 재작성
- TOML 설정에서 `BrowserConfig` 직접 임베드

---

## 5. 요구사항 체크리스트

| # | 요구사항 | oxibrowser 변경 | OS에 미치는 영향 |
|---|---------|----------------|-----------------|
| 1 | `BrowserConfig`에 `Deserialize`/`Serialize` | config.rs derive 추가 | TOML 직접 파싱 |
| 2 | `BrowseResult` 구조체 | 신규 | 통합 반환값 |
| 3 | `Browser::browse()` 원샷 | Browser에 메서드 1개 | 90% 케이스 1-call |
| 4 | `Tab` 래퍼 | 신규 (`Arc<Mutex<Session>>`) | `&self` 전용, OS lock 제거 |
| 5 | `Browser::new_tab()` | Browser에 메서드 1개 | 인터랙티브 세션 |
| 6 | `Tab::click()`/`type()` 내장 | js/input.rs 활용 | OS가 JS 안 짬 |
| 7 | 탐색 메서드 `BrowseResult` 반환 | 반환값 변경 | page() 체인 불필요 |
