# OxiBrowser CLI 2.0 — 남은 작업 설계

> Phase 1 완료. Phase 2~4 + 기술부채 정리.

## 현재 상태

| Phase | 상태 | 내용 |
|-------|------|------|
| Phase 1 | ✅ 완료 | fetch, extract, run, describe, skill, serve, version + 입력검증 + JSON 래퍼 |
| Phase 2 | 🔲 | `session` stdin/stdout JSON REPL |
| Phase 3 | 🔲 | run 강화 + 통합 테스트 |
| Phase 4 | 🔲 | MCP 래핑 (선택) |
| 기술부채 | 🔲 | dead code, 문서 업데이트 |

---

## 0. 기술부채 (1일)

### 0.1 dead code 정리

```
crates/oxibrowser/src/output.rs:
  - with_meta()            → 삭제 (success_with_meta로 대체됨)
  - print_human_result()   → 삭제 또는 run 향상 시 사용
```

### 0.2 설계 문서 업데이트

```
docs/CLI-V2-DESIGN.md:
  - --human 플래그 제거 반영
  - 기본 format = markdown 반영
  - human-first 원칙 명시
```

---

## 1. Phase 2: `session` (2~3주)

### 1.1 개념

에이전트가 subprocess로 `oxibrowser session --json` 실행.
stdin에 명령 → stdout에 JSON. 프로세스가 살아있는 동안 탭이 메모리에 유지.

```
에이전트 ──stdin──→ "new"
         ←─stdout── {"ok":true,"data":{"tab_id":"t1"}}

에이전트 ──stdin──→ "goto t1 https://example.com"
         ←─stdout── {"ok":true,"data":{"url":"...","title":"...","status":200},"meta":{"tab_id":"t1","elapsed_ms":142}}

에이전트 ──stdin──→ "click t1 button"
         ←─stdout── {"ok":true,"meta":{"tab_id":"t1"}}

에이전트 ──stdin──→ "exit"
         ←─stdout── (프로세스 종료, exit 0)
```

### 1.2 아키텍처

```
crates/oxibrowser/src/
├── main.rs              # Commands::Session → run_session()
└── session/
    ├── mod.rs            # 진입점, 이벤트 루프
    ├── parser.rs         # 명령 텍스트 → SessionCommand
    ├── executor.rs       # SessionCommand → Tab 메서드 호출 → CliResponse
    └── tab_manager.rs    # HashMap<String, Tab> + ID 생성
```

### 1.3 핵심 타입

```rust
// session/parser.rs

/// 세션 명령
enum SessionCommand {
    New,
    Goto { tab_id: String, url: String, wait: Option<String>, timeout: Option<u64> },
    Back { tab_id: String },
    Forward { tab_id: String },
    Reload { tab_id: String },
    Click { tab_id: String, selector: String },
    Fill { tab_id: String, selector: String, value: String },
    Press { tab_id: String, key: String },
    Type { tab_id: String, selector: String, text: String },
    Select { tab_id: String, selector: String, value: String },
    Check { tab_id: String, selector: String },
    Uncheck { tab_id: String, selector: String },
    Scroll { tab_id: String, delta_x: f64, delta_y: f64 },
    Eval { tab_id: String, expression: String, await_expr: bool },
    Extract { tab_id: String, selector: Option<String>, all: bool, attrs: Vec<String>,
              links: bool, title: bool, text: bool, markdown: bool, max_bytes: Option<u64> },
    Content { tab_id: String, format: String, max_bytes: Option<u64> },
    Screenshot { tab_id: String, output: Option<String>, width: Option<u32> },
    Wait { tab_id: String, selector: String, timeout: Option<u64> },
    Close { tab_id: String },
    CloseAll,
    List,
    Help,
    Exit,
}

/// 명령 파싱 결과
type ParseResult = Result<SessionCommand, String>;

/// "goto t1 https://example.com --wait .content" → SessionCommand::Goto { ... }
fn parse_session_command(line: &str) -> ParseResult;
```

```rust
// session/tab_manager.rs

struct TabManager {
    tabs: HashMap<String, Tab>,
    next_id: u32,
}

impl TabManager {
    fn new() -> Self;
    fn create_tab(&mut self, browser: &Browser) -> Result<String>;       // → "t1", "t2", ...
    fn get_tab(&self, tab_id: &str) -> Option<&Tab>;
    fn get_tab_mut(&mut self, tab_id: &str) -> Option<&mut Tab>;
    fn close_tab(&mut self, tab_id: &str) -> Result<()>;
    fn close_all(&mut self) -> Result<()>;
    fn list(&self) -> Vec<TabInfo>;
}
```

```rust
// session/executor.rs

async fn execute(
    cmd: SessionCommand,
    browser: &Browser,
    manager: &mut TabManager,
) -> CliResponse;
```

### 1.4 이벤트 루프

```rust
// session/mod.rs

pub async fn run_session() -> i32 {
    let browser = Browser::new(BrowserConfig::headless()).await
        .expect("browser init failed");
    let mut manager = TabManager::new();
    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();
    let stdout = Mutex::new(stdout);

    // SIGTERM 핸들러
    let terminated = Arc::new(AtomicBool::new(false));
    // ... signal_hook 설정 ...

    for line in stdin.lines() {
        // SIGTERM 체크
        if terminated.load(Ordering::Relaxed) { break; }

        let line = match line {
            Ok(l) => l,
            Err(_) => break,  // EOF
        };

        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        // 파싱
        let cmd = match parse_session_command(line) {
            Ok(c) => c,
            Err(e) => {
                let resp = CliResponse::error(&e, "PARSE_ERROR");
                writeln!(stdout.lock(), "{}", serde_json::to_string(&resp).unwrap()).ok();
                continue;
            }
        };

        // exit 체크
        if matches!(cmd, SessionCommand::Exit) {
            break;
        }

        // 실행
        let resp = execute(cmd, &browser, &mut manager).await;
        writeln!(stdout.lock(), "{}", serde_json::to_string(&resp).unwrap()).ok();
    }

    // 정리
    manager.close_all().await.ok();
    browser.close().await.ok();
    0
}
```

### 1.5 명령 파서 설계

플레인 텍스트 한 줄 → `SessionCommand`. 최소한의 파싱.

```
new
goto <tab_id> <url> [--wait <selector>] [--timeout <ms>]
back <tab_id>
forward <tab_id>
reload <tab_id>
click <tab_id> <selector>
fill <tab_id> <selector> <value>
press <tab_id> <key>
type <tab_id> <selector> <text>
select <tab_id> <selector> <value>
check <tab_id> <selector>
uncheck <tab_id> <selector>
scroll <tab_id> <dx> <dy>
eval <tab_id> <expression> [--await]
extract <tab_id> [--selector <s>] [--all] [--attrs a,b] [--links] [--title] [--text] [--markdown] [--max-bytes N]
content <tab_id> [--format markdown|html|text] [--max-bytes N]
screenshot <tab_id> [-o path] [--width N]
wait <tab_id> <selector> [--timeout <ms>]
close <tab_id>
close --all
list
help
exit
```

파싱 전략:
- `line.split_whitespace().collect::<Vec<_>>()` 로 토큰화
- 첫 토큰 = 명령어 이름 → `match` 로 분기
- 나머지 토큰 = positional args + `--flag value` 쌍
- `extract`/`content` 플래그 파싱은 기존 `clap` 정의 재사용하지 않고 간단한 수동 파싱

### 1.6 에러 처리

```json
{"ok":false,"error":"tab not found: t3","error_code":"TAB_NOT_FOUND","meta":{"tab_id":"t3"}}
{"ok":false,"error":"no element matching '.nonexistent'","error_code":"DOM_NOT_FOUND","meta":{"tab_id":"t1"}}
{"ok":false,"error":"unknown command: bla","error_code":"PARSE_ERROR"}
```

에러 코드 추가:

| error_code | 의미 |
|---|---|
| `TAB_NOT_FOUND` | tab_id가 존재하지 않음 |
| `TAB_CLOSED` | 탭이 이미 닫힘 |
| `PARSE_ERROR` | 명령 문법 오류 |
| `DOM_NOT_FOUND` | 셀렉터 매치 없음 |
| `JS_ERROR` | JS 평가 에러 |

### 1.7 EOF / SIGTERM

```
stdin EOF (에이전트 프로세스 종료):
  → for line in stdin.lines()가 Err 반환
  → break → close_all() → browser.close() → exit 0

SIGTERM:
  → signal_hook이 AtomicBool 설정
  → 다음 루프 반복에서 체크 → break → 동일 정리

비정상 종료 (kill -9):
  → 프로세스 죽으면 OS가 fds 정리. 열려있는 Browser Drop이 정리.
```

### 1.8 Tab API 매핑

| 세션 명령 | Tab 메서드 | 비고 |
|---|---|---|
| `new` | `Browser::new_tab()` | tab_id 자동 생성 |
| `goto` | `tab.goto(url)` | → BrowseResult |
| `back` | `tab.back()` | → BrowseResult |
| `forward` | `tab.forward()` | → BrowseResult |
| `reload` | `tab.reload()` | → BrowseResult |
| `click` | `tab.click(selector)` | |
| `fill` | `tab.fill(selector, value)` | |
| `press` | `tab.press(combo)` | "Enter", "Tab", "Ctrl+A" |
| `type` | `tab.r#type(selector, text)` | 글자 하나씩 |
| `select` | `tab.select_option(selector, value)` | |
| `check` | `tab.check(selector)` | |
| `uncheck` | `tab.uncheck(selector)` | |
| `scroll` | `tab.scroll(dx, dy)` | |
| `eval` | `tab.evaluate(expr)` 또는 `evaluate_await` | |
| `wait` | `tab.wait_for(selector, timeout_ms)` | |
| `screenshot` | `tab.screenshot(width)` | stdout에 base64 또는 -o 파일 |
| `close` | `tab.close()` | |

### 1.9 `extract`/`content` 재사용

세션 내부의 `extract`, `content` 명령은 기존 `run_extract`, `fetch_direct` 로직을
그대로 재사용하되 URL을 새로 fetch하지 않고 기존 탭의 Page를 사용.

```rust
// 핵심: Tab에서 Page 가져오기
let session = tab.session.read().await;
let page = session.page();
let doc = page.root_frame().document();

// 이후 기존 extract 로직 동일
```

이를 위해 `run_extract`/`fetch_direct`를 리팩토링:
- `extract_from_page(page, selector, ...)` 함수로 분리
- 기존 CLI는 `browser.new_page(url)` 후 호출
- 세션은 `tab.session`에서 가져온 page로 호출

### 1.10 파일 구조 및 예상 규모

```
crates/oxibrowser/src/session/
├── mod.rs           (~80줄)  이벤트 루프, 진입점
├── parser.rs        (~200줄) 명령 텍스트 → SessionCommand
├── executor.rs      (~300줄) SessionCommand → CliResponse
└── tab_manager.rs   (~60줄)  HashMap<String, Tab> 관리
```

총 ~640줄. 기존 Tab API를 그대로 사용하므로 새로 짜야 할 것은
명령 파서 + 라우터 + stdin/stdout 루프뿐.

### 1.11 테스트 전략

```
1. 파서 단위 테스트
   - "new" → SessionCommand::New
   - "goto t1 https://example.com" → SessionCommand::Goto { tab_id: "t1", url: "...", ... }
   - "click t1 button" → SessionCommand::Click { ... }
   - "bla bla" → Err("unknown command: bla")

2. TabManager 단위 테스트
   - create_tab → tab_id 생성
   - get_tab → Some/None
   - close_tab → 제거 확인

3. 통합 테스트 (cargo test --ignored)
   - subprocess로 oxibrowser session --json 실행
   - stdin에 명령 보내고 stdout에서 JSON 읽기
   - new → goto → click → extract → close → exit 플로우

4. EOF/SIGTERM 테스트
   - stdin 닫기 → 정상 종료 확인
   - SIGTERM 보내기 → 정상 종료 확인
```

---

## 2. Phase 3: `run` 강화 + 통합 테스트 (1주)

### 2.1 run 결과 JSON 통일

현재 `run` 출력은 ScriptResult를 직접 JSON 직렬화.
이걸 표준 CliResponse 래퍼로 통일:

```json
// 현재
{"name":"test","steps":[...],"vars":{},"success":true,"duration_ms":42}

// 목표
{
  "ok": true,
  "data": {
    "name": "test",
    "steps": [...],
    "vars": {},
    "success": true,
    "duration_ms": 42
  },
  "meta": {"elapsed_ms": 42}
}
```

변경: `run_script`에서 `CliResponse::success(data)` 사용.

### 2.2 describe에 세션 명령 스키마 추가

```json
// oxibrowser describe session --json
{
  "ok": true,
  "data": {
    "description": "Interactive session (stdin/stdout JSON REPL)",
    "session_commands": {
      "new": { "description": "Create a new tab" },
      "goto": { "args": ["tab_id", "url"], "flags": ["wait", "timeout"] },
      "click": { "args": ["tab_id", "selector"] },
      "fill": { "args": ["tab_id", "selector", "value"] },
      "press": { "args": ["tab_id", "key"] },
      "eval": { "args": ["tab_id", "expression"], "flags": ["await"] },
      "extract": { "args": ["tab_id"], "flags": ["selector", "all", "attrs", "links", "title", "text", "markdown", "max-bytes"] },
      "content": { "args": ["tab_id"], "flags": ["format", "max-bytes"] },
      "screenshot": { "args": ["tab_id"], "flags": ["output", "width"] },
      "wait": { "args": ["tab_id", "selector"], "flags": ["timeout"] },
      "close": { "args": ["tab_id"] },
      "list": {},
      "exit": {}
    }
  }
}
```

### 2.3 skill 가이드 업데이트

session이 구현되면 skill 출력의 session 섹션을 실제 동작 기반으로 업데이트.

### 2.4 통합 테스트

```
tests/cli_integration.rs (또는 tests/ 디렉토리):

1. one-shot 모드
   - fetch + --json → ok=true
   - fetch + --format text → 텍스트에 줄바꿈
   - fetch + --summary → meta 데이터
   - fetch + --fields → 필드 필터링
   - fetch + --max-bytes → 잘림
   - extract + --links → 링크 배열
   - extract + --selector a --attrs text,href → 구조화
   - 에러: ftp → exit 2
   - 에러: 존재X 도메인 → exit 4

2. session 모드
   - new → goto → content → close → exit
   - new → goto → click → wait → extract
   - 존재X tab_id → TAB_NOT_FOUND
   - EOF → 정상 종료

3. run 모드
   - 간단한 2-step 스크립트
   - 에러: 잘못된 YAML
```

---

## 3. Phase 4: MCP 래핑 (선택, 1~2주)

### 3.1 개념

`session`의 stdin/stdout JSON을 MCP (Model Context Protocol)로 래핑.
에이전트가 MCP 클라이언트로 OxiBrowser를 도구로 사용.

### 3.2 두 가지 진입점

```
# 옵션 A: 전용 명령
oxibrowser mcp
→ stdin/stdout에 MCP 프로토콜 (JSON-RPC)
→ tools: { fetch, extract, session_new, session_goto, ... }

# 옵션 B: serve에 stdio 모드 추가
oxibrowser serve --stdio
→ 기존 CDP 서버에 MCP stdio 모드 추가
```

### 3.3 MCP 도구 맵

```json
{
  "tools": [
    {
      "name": "oxibrowser_fetch",
      "description": "Fetch a URL and return content",
      "inputSchema": {
        "url": "string",
        "format?": "markdown|html|text",
        "maxBytes?": "number",
        "fields?": "string",
        "summary?": "boolean"
      }
    },
    {
      "name": "oxibrowser_extract",
      "description": "Extract structured data from a URL",
      "inputSchema": {
        "url": "string",
        "selector?": "string",
        "attrs?": "string",
        "links?": "boolean"
      }
    },
    {
      "name": "oxibrowser_session_new",
      "description": "Create a new browser tab"
    },
    {
      "name": "oxibrowser_session_goto",
      "inputSchema": { "tabId": "string", "url": "string" }
    },
    {
      "name": "oxibrowser_session_click",
      "inputSchema": { "tabId": "string", "selector": "string" }
    },
    ...
  ]
}
```

### 3.4 구현 전략

Phase 2의 `session` executor를 그대로 재사용.
MCP 레이어는 JSON-RPC ←→ SessionCommand 변환기.

```
MCP JSON-RPC → 파싱 → SessionCommand → executor → CliResponse → MCP JSON-RPC
```

session/executor.rs가 프로토콜에 독립적이므로, MCP는 얇은 래퍼.

---

## 4. 우선순위 및 타임라인

```
순서   작업                    예상 기간   의존성
────   ────                    ─────────   ──────
0      기술부채 정리            1일         없음
2      session 구현            2~3주       Tab API (이미 있음)
3      run 강화 + 통합테스트    1주         session
4      MCP 래핑                1~2주       session (선택)
```

**추천**: 0 → 2 → 3 순서. Phase 4는 실제 수요가 생기면 시작.

### 각 Phase의 완료 기준

| Phase | 완료 기준 |
|-------|----------|
| 0 | warning 0개, 문서 최신화 |
| 2 | `oxibrowser session --json`으로 에이전트가 탭 생성→조작→추출→종료 가능 |
| 3 | 통합 테스트 3종 (one-shot, session, run) 통과 |
| 4 | MCP 클라이언트가 OxiBrowser를 도구로 호출 가능 |
