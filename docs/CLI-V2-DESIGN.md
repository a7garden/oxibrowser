# OxiBrowser CLI 2.0 — Agent-First Redesign

> **원칙**: OxiBrowser는 단일 정적 바이너리다. 서버 없이, 데몬 없이, 그냥 동작한다.

## 1. 핵심 모델

에이전트가 OxiBrowser에 하는 일은 3가지뿐이다:

```
패턴 A: "이 URL 읽어줘"                        (80%)
  → oxibrowser fetch <url> --format markdown --json
  → 단일 프로세스, 결과 출력, 끝

패턴 B: "이것저것 자동으로 해줘"                  (15%)
  → oxibrowser run script.yaml
  → 단일 프로세스 안에서 탭 생성→조작→추출→종료

패턴 C: "페이지 읽고 → 판단하고 → 또 조작하고"    (5%)
  → oxibrowser session --json
  → subprocess로 실행, stdin에 명령, stdout에 JSON
  → 에이전트가 중간에 판단하고 다음 명령 결정
```

서버 없이, 데몬 없이, 소켓 없이, PID 파일 없이.

---

## 2. 명령어 맵 — 7개 서브커맨드

```
oxibrowser
├── fetch <url> [flags]          # one-shot: URL → 콘텐츠
├── extract <url> [flags]        # one-shot: URL → 구조화된 데이터
├── run <script.yaml>            # batch: YAML 스크립트 실행
├── session [flags]              # interactive: stdin/stdout JSON REPL
├── serve [flags]                # CDP 서버 (Puppeteer/Playwright용)
├── describe [command] [flags]   # 스키마 인트로스펙션
├── skill                        # 에이전트 가이드 출력
└── version                      # 버전 정보
```

### 2.1 각 명령의 역할

| 명령 | 상태 | 용도 |
|------|------|------|
| `fetch` | stateless | URL 하나 읽고 결과 반환 |
| `extract` | stateless | URL 하나에서 구조화된 데이터 추출 |
| `run` | self-contained | YAML 스크립트 안에서 자체 탭 관리 |
| `session` | stateful | stdin/stdout REPL, 에이전트가 직접 탭 제어 |
| `serve` | stateful | CDP WebSocket 서버 (기존 그대로) |
| `describe` | stateless | CLI 자기 설명 (에이전트용) |
| `skill` | stateless | 에이전트 컨텍스트용 가이드 출력 |

### 2.2 기존 명령 변경 사항

| 기존 (1.0) | 변경 (2.0) | 이유 |
|---|---|---|
| `fetch <url>` | 그대로 | 핵심 명령 |
| `extract <url>` | 그대로 | 핵심 명령 |
| `eval <url> <expr>` | `fetch <url> --eval <expr>` | eval 이중 정의 제거 |
| `browse <url>` | `fetch <url> --click/--fill/--wait/...` | fetch로 흡수 |
| `serve` | 그대로 | CDP 호환용 |
| `run <script>` | 그대로 | 자동화용 |
| `version` | 그대로 | — |
| *(신규)* | `session` | 상태 저장 REPL |
| *(신규)* | `describe` | 스키마 인트로스펙션 |
| *(신규)* | `skill` | 에이전트 가이드 |

### 2.3 `browse` 제거, `fetch`로 흡수

기존 `browse`는 "fetch + 상호작용"이었다. 2.0에서는 `fetch`에 상호작용 플래그를 넣는다:

```bash
# 기존:
oxibrowser browse <url> --click "button" --wait ".result" --extract "h3" --format markdown

# 2.0:
oxibrowser fetch <url> \
  --click "button" \
  --wait ".result" \
  --extract "h3" \
  --format markdown \
  --json
```

의미는 같다. `fetch`가 "URL 가져와서 뭔가 하고 결과 내주는" 명령이라는 건 변하지 않는다.

---

## 3. `session` — 상태 저장 REPL

### 3.1 개념

에이전트가 `oxibrowser session --json`을 subprocess로 실행한다.
프로세스가 살아있는 동안 탭이 메모리에 유지된다.

```
에이전트 프로세스                     oxibrowser session 프로세스
────────────────                     ──────────────────────────

stdin  ──"new"─────────────────────→  Tab 생성
stdout ←──{"ok":true,"data":...}────  결과 출력

stdin  ──"goto t1 https://..."─────→  탭에서 네비게이션
stdout ←──{"ok":true,"data":...}────  결과 출력

stdin  ──"click t1 button"─────────→  탭에서 클릭
stdout ←──{"ok":true}───────────────  결과 출력

stdin  ──"extract t1 --links"──────→  탭에서 추출
stdout ←──{"ok":true,"data":...}────  결과 출력

stdin  ──"exit"────────────────────→  프로세스 종료
```

### 3.2 세션 명령어

`session` 안에서 사용하는 내부 명령들:

```
new                                     # 새 탭 생성 → tab_id
goto <tab_id> <url> [--wait <sel>]      # 네비게이션
back <tab_id>                           # 뒤로
forward <tab_id>                        # 앞으로
reload <tab_id>                         # 새로고침

click <tab_id> <selector>               # 클릭
fill <tab_id> <selector> <value>        # 입력
press <tab_id> <key>                    # 키 입력
type <tab_id> <selector> <text>         # 타이핑 (글자 하나씩)
select <tab_id> <selector> <value>      # 셀렉트 박스
check <tab_id> <selector>               # 체크박스 체크
scroll <tab_id> [--down N] [--up N]     # 스크롤

eval <tab_id> <expression> [--await]    # JS 평가
extract <tab_id> [flags]                # 데이터 추출
content <tab_id> [--format F]           # 페이지 콘텐츠
screenshot <tab_id> [-o path]           # 스크린샷

close <tab_id>                          # 탭 닫기
close --all                             # 모든 탭 닫기
list                                    # 활성 탭 목록
help                                    # 세션 명령 도움말
exit                                    # 세션 종료 (프로세스 종료)
```

### 3.3 프로토콜: 줄 단위 JSON

입력: 한 줄에 하나의 명령 (플레인 텍스트).

```
goto t1 https://example.com --wait ".content"
```

출력: 한 줄에 하나의 JSON.

```json
{"ok":true,"data":{"url":"https://example.com","title":"Example Domain","status":200},"meta":{"tab_id":"t1","elapsed_ms":142}}
```

에러:

```json
{"ok":false,"error":"no element matching '.nonexistent'","error_code":"DOM_NOT_FOUND","meta":{"tab_id":"t1","elapsed_ms":3}}
```

### 3.4 에이전트 워크플로우 예시

```
에이전트: "구글에서 Rust 검색 결과 가져와"

→ (subprocess 생성) oxibrowser session --json

→ new
← {"ok":true,"data":{"tab_id":"t1"}}

→ goto t1 https://google.com
← {"ok":true,"data":{"url":"https://www.google.com","title":"Google","status":200}}

→ fill t1 "input[name=q]" "Rust"
← {"ok":true}

→ press t1 Enter
← {"ok":true}

→ wait t1 "#search" --timeout 5000
← {"ok":true,"data":{"elapsed_ms":420}}

→ extract t1 --selector "h3" --all --max-bytes 4000
← {"ok":true,"data":{"items":["Rust Programming Language","Learn Rust","..."],"count":8,"truncated":false}}

→ (에이전트가 판단: "Rust Programming Language 링크를 클릭하자")

→ click t1 "h3 a"
← {"ok":true}

→ content t1 --format markdown --max-bytes 8000
← {"ok":true,"data":{"url":"https://www.rust-lang.org","title":"Rust","markdown":"# Rust\n\nA language empowering everyone...","truncated":false}}

→ close t1
← {"ok":true}

→ exit
← (프로세스 종료)
```

### 3.5 `wait` 명령

별도 명령으로 분리. `goto --wait`로도 가능하지만, 상호작용 후에도 기다려야 하는 경우가 많다:

```bash
# goto와 함께:
goto t1 https://example.com --wait ".content"

# 클릭 후에:
click t1 "button.load-more"
wait t1 ".new-items" --timeout 3000
```

### 3.6 EOF / SIGTERM 처리

- stdin이 닫히면 (EOF) → 모든 탭 닫고 정상 종료
- SIGTERM 받으면 → 모든 탭 닫고 정상 종료
- 탭이 닫히지 않으면 → 5초 후 강제 종료

에이전트가 프로세스를 죽이기만 해도 리소스가 정리된다.

### 3.7 구현: `session`은 `main.rs` 안에

새 모듈이 필요하지 않다. `session` 서브커맨드가 하는 일:

```rust
Commands::Session { json } => {
    let browser = Browser::new(BrowserConfig::headless()).await?;
    let mut tabs: HashMap<String, Tab> = HashMap::new();
    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();

    for line in stdin.lines() {
        let line = line?;
        let cmd = parse_session_command(&line)?;
        let result = execute_session_command(&browser, &mut tabs, cmd).await;
        writeln!(stdout, "{}", serde_json::to_string(&result)?)?;
        stdout.flush()?;
        if cmd.is_exit() { break; }
    }

    // 정리
    for (_, tab) in tabs { let _ = tab.close().await; }
    browser.close().await?;
}
```

핵심 로직은 기존 `Tab` 구조체를 그대로 사용한다. 새로 짤 건 **명령 파서 + 라우터**뿐이다.

---

## 4. 출력 형식

### 4.1 출력 결정 규칙

```
--json 플래그 있음        → JSON
stdout이 파이프/리다이렉트 → JSON (자동)
stdout이 TTY              → 사람이 읽을 수 있는 형식
```

`session` 모드는 항상 JSON (stdin/stdout이므로).

### 4.2 표준 응답 래퍼

모든 JSON 출력은 동일한 구조:

```json
{
  "ok": true,
  "data": { ... },
  "meta": {
    "tab_id": "t1",
    "elapsed_ms": 142
  }
}
```

에러:

```json
{
  "ok": false,
  "error": "no element matching 'button#submit'",
  "error_code": "DOM_NOT_FOUND",
  "meta": {
    "tab_id": "t1",
    "elapsed_ms": 3
  }
}
```

- `ok`: 성공 여부 (bool)
- `data`: 성공 시 결과 (에러 시 null)
- `error`: 실패 시 사람 읽을 수 있는 메시지 (성공 시 생략)
- `error_code`: 실패 시 머신 판독 가능한 코드 (성공 시 생략)
- `meta`: 항상 존재. `tab_id`는 관련 탭이 있을 때만, `elapsed_ms`는 항상

### 4.3 exit code 체계

| 코드 | 의미 |
|------|------|
| 0 | 성공 (또는 `--max-bytes`로 잘린 경우도 성공) |
| 1 | 런타임 에러 (DOM 없음, JS 에러, 요소 못 찾음) |
| 2 | 입력 검증 실패 (잘못된 URL, 제어 문자, 경로 순회) |
| 3 | 타임아웃 |
| 4 | 네트워크 에러 |

### 4.4 에러 코드 체계

| error_code | exit | 의미 |
|------------|------|------|
| `INVALID_URL` | 2 | URL 형식 오류 |
| `INVALID_SELECTOR` | 2 | CSS selector 문법 오류 |
| `DOM_NOT_FOUND` | 1 | 요소를 찾을 수 없음 |
| `JS_ERROR` | 1 | JS 평가 에러 |
| `TIMEOUT` | 3 | 타임아웃 |
| `NETWORK_ERROR` | 4 | 네트워크 오류 (DNS, 연결 거부 등) |
| `HTTP_ERROR` | 4 | HTTP 4xx/5xx |
| `TAB_NOT_FOUND` | 1 | tab_id가 존재하지 않음 |
| `TAB_CLOSED` | 1 | 탭이 이미 닫힘 |
| `INPUT_VALIDATION` | 2 | 제어 문자, null 바이트 등 |
| `PATH_TRAVERSAL` | 2 | 출력 경로가 CWD 밖 |
| `SSRF_BLOCKED` | 2 | SSRF 필터 차단 |
| `TRUNCATED` | 0 | `--max-bytes`로 잘림 (ok=true, data에 truncated=true) |

---

## 5. 컨텍스트 윈도우 관리

### 5.1 `--max-bytes` (모든 출력 명령)

```bash
oxibrowser fetch <url> --format markdown --max-bytes 4096 --json
```

```json
{
  "ok": true,
  "data": {
    "url": "https://example.com",
    "title": "Example",
    "status": 200,
    "markdown": "# Example\n\nThis domain is for use in illustrative examples...",
    "truncated": true,
    "total_bytes": 15420,
    "returned_bytes": 4096
  },
  "meta": {"elapsed_ms": 42}
}
```

동작: `data` 안의 문자열 필드(markdown, html, body, text 등)를 합산하여 `--max-bytes`를 넘으면 잘라낸다. `truncated`와 `total_bytes`로 에이전트가 "더 읽어야 하나?"를 판단한다.

### 5.2 `--fields` (content, fetch)

응답에서 특정 필드만 포함:

```bash
# URL, 타이틀, 상태만 (markdown/html은 생략 → 토큰 절약)
oxibrowser fetch <url> --fields url,title,status --json
```

```json
{
  "ok": true,
  "data": {
    "url": "https://example.com",
    "title": "Example Domain",
    "status": 200
  }
}
```

가능한 필드: `url`, `title`, `status`, `markdown`, `html`, `text`, `content_type`

### 5.3 `--attrs` (extract)

DOM 요소에서 특정 속성만 추출:

```bash
# 링크의 텍스트와 href만
oxibrowser extract <url> --selector "a" --all --attrs text,href --json
```

```json
{
  "ok": true,
  "data": {
    "count": 3,
    "items": [
      {"text": "Example", "href": "https://example.com"},
      {"text": "Rust", "href": "https://rust-lang.org"},
      {"text": "GitHub", "href": "https://github.com"}
    ],
    "truncated": false
  }
}
```

`--attrs` 생략 시 기본값: `text`만.

### 5.4 `--summary` (fetch, content)

전체 콘텐츠 대신 페이지 메타데이터만:

```bash
oxibrowser fetch <url> --summary --json
```

```json
{
  "ok": true,
  "data": {
    "url": "https://example.com",
    "title": "Example Domain",
    "status": 200,
    "content_type": "text/html",
    "headings": ["Example Domain", "More information..."],
    "links_count": 1,
    "forms_count": 0,
    "images_count": 0,
    "text_length": 243
  }
}
```

에이전트가 전체 페이지를 읽기 전에 "이 페이지가 내가 원하는 건가?" 판단.

---

## 6. 스키마 인트로스펙션

### 6.1 `describe` 명령

```bash
# 7개 명령 요약 (에이전트가 한 번에 읽을 수 있음)
oxibrowser describe --json
```

```json
{
  "ok": true,
  "data": {
    "name": "oxibrowser",
    "version": "0.11.0",
    "commands": {
      "fetch": {
        "description": "Fetch a URL and return content in one shot",
        "usage": "oxibrowser fetch <url> [flags]",
        "args": ["url"],
        "flags": {
          "format": {"type": "enum", "values": ["html", "markdown", "text"], "default": "html"},
          "json": {"type": "bool"},
          "max-bytes": {"type": "int", "description": "Truncate output at N bytes"},
          "fields": {"type": "string", "description": "Comma-separated fields to include"},
          "summary": {"type": "bool", "description": "Page metadata only"},
          "eval": {"type": "string", "description": "Evaluate JS after page load"},
          "click": {"type": "string", "description": "Click element matching selector"},
          "fill": {"type": "string", "format": "selector:value"},
          "press": {"type": "string", "description": "Press key after navigation"},
          "wait": {"type": "string", "description": "Wait for selector before output"},
          "extract": {"type": "string", "description": "Extract text from selector"},
          "timeout": {"type": "int", "default": 30, "unit": "seconds"}
        }
      },
      "extract": {
        "description": "Extract structured data from a URL",
        "usage": "oxibrowser extract <url> [flags]",
        "args": ["url"],
        "flags": {
          "selector": {"type": "string"},
          "all": {"type": "bool"},
          "links": {"type": "bool"},
          "title": {"type": "bool"},
          "text": {"type": "bool"},
          "markdown": {"type": "bool"},
          "attrs": {"type": "string", "description": "Comma-separated attributes to extract"},
          "max-bytes": {"type": "int"},
          "json": {"type": "bool"},
          "timeout": {"type": "int", "default": 30}
        }
      },
      "run": {
        "description": "Run a YAML browser automation script",
        "usage": "oxibrowser run <script.yaml>",
        "args": ["script"]
      },
      "session": {
        "description": "Start interactive session (stdin/stdout JSON REPL)",
        "usage": "oxibrowser session [--json]",
        "session_commands": [
          "new", "goto", "back", "forward", "reload",
          "click", "fill", "press", "type", "select", "check", "scroll",
          "eval", "extract", "content", "screenshot",
          "wait", "close", "list", "help", "exit"
        ]
      },
      "serve": {
        "description": "Start CDP server for Puppeteer/Playwright",
        "usage": "oxibrowser serve [flags]",
        "flags": {
          "host": {"type": "string", "default": "127.0.0.1"},
          "port": {"type": "int", "default": 9222},
          "cookie-file": {"type": "string"}
        }
      },
      "describe": {
        "description": "Print CLI schema as JSON",
        "usage": "oxibrowser describe [command] [--compact] [--json]"
      },
      "skill": {
        "description": "Print agent skill guide (markdown)",
        "usage": "oxibrowser skill"
      }
    }
  }
}
```

### 6.2 축약 버전

```bash
oxibrowser describe --compact --json
```

```json
{
  "fetch": {"args": ["url"], "flags": ["format","json","max-bytes","fields","summary","eval","click","fill","press","wait","extract"]},
  "extract": {"args": ["url"], "flags": ["selector","all","links","title","text","attrs","max-bytes","json"]},
  "run": {"args": ["script"]},
  "session": {"commands": ["new","goto","click","fill","press","eval","extract","content","screenshot","close","list","exit"]},
  "serve": {"flags": ["host","port"]},
  "describe": {},
  "skill": {}
}
```

~200 토큰. 에이전트가 한 번에 컨텍스트에 넣을 수 있다.

---

## 7. 입력 검증

### 7.1 검증 함수

```rust
mod validate {
    /// 제어 문자 거부 (에이전트 할루시네이션 방지)
    pub fn reject_control_chars(input: &str) -> Result<&str> {
        if input.chars().any(|c| (c < '\x20' && c != '\n' && c != '\r' && c != '\t') || c == '\x7f') {
            return Err(InputError::ControlChars);
        }
        Ok(input)
    }

    /// 경로 순회 방지 (screenshot, 파일 출력)
    pub fn safe_output_path(path: &str, base: &Path) -> Result<PathBuf> {
        let resolved = base.join(path);
        // canonicalize parent (파일이 아직 없을 수 있으므로)
        let parent = resolved.parent().unwrap_or(base);
        let canonical_parent = parent.canonicalize()
            .map_err(|_| InputError::PathTraversal)?;
        if !canonical_parent.starts_with(base.canonicalize().unwrap_or_default()) {
            return Err(InputError::PathTraversal);
        }
        Ok(resolved)
    }

    /// URL 검증 (http/https만)
    pub fn validate_url(url: &str) -> Result<Url> {
        let parsed = Url::parse(url)?;
        match parsed.scheme() {
            "http" | "https" => Ok(parsed),
            _ => Err(InputError::InvalidScheme),
        }
    }

    /// CSS selector 기본 검증
    pub fn validate_selector(selector: &str) -> Result<&str> {
        if selector.trim().is_empty() {
            return Err(InputError::EmptySelector);
        }
        if selector.contains('\0') {
            return Err(InputError::NullByte);
        }
        reject_control_chars(selector)?;
        Ok(selector)
    }

    /// JS expression 기본 검증
    pub fn validate_expression(expr: &str) -> Result<&str> {
        if expr.contains('\0') {
            return Err(InputError::NullByte);
        }
        reject_control_chars(expr)?;
        Ok(expr)
    }
}
```

### 7.2 검증 적용 지점

| 입력 | 검증 |
|------|------|
| `<url>` | URL 파싱 + 스킴 검증 + SSRF 필터 |
| `<selector>` | null 바이트, 빈 문자열, 제어 문자 |
| `<expression>` | null 바이트, 제어 문자 |
| `--screenshot -o <path>` | 경로 순회, CWD 샌드박싱 |
| 모든 문자열 인자 | 제어 문자 거부 |

### 7.3 검증 에러는 항상 exit code 2

```json
{"ok":false,"error":"control character detected in selector","error_code":"INPUT_VALIDATION"}
```

에이전트가 exit code 2를 보면 "내 입력이 잘못됐다"고 바로 안다.

---

## 8. `fetch` 명령 — 전체 사양

`fetch`는 one-shot의 핵심이다. URL을 받아서 결과를 내주는 모든 일을 한다.

### 8.1 사용법

```bash
oxibrowser fetch <url> [flags]
```

### 8.2 플래그

| 플래그 | 타입 | 기본값 | 설명 |
|-------|------|--------|------|
| `--format` | enum | `html` | 출력 형식: `html`, `markdown`, `text` |
| `--json` | bool | false | JSON으로 출력 |
| `--max-bytes` | int | 없음 | 출력 바이트 제한 |
| `--fields` | string | 전체 | 포함할 필드 (쉼표 구분) |
| `--summary` | bool | false | 페이지 메타데이터만 |
| `--eval` | string | 없음 | 페이지 로드 후 JS 평가 |
| `--click` | string | 없음 | 요소 클릭 |
| `--fill` | string | 없음 | 입력 채우기 (`selector:value`) |
| `--press` | string | 없음 | 키 입력 |
| `--wait` | string | 없음 | 출력 전까지 셀렉터 대기 |
| `--wait-timeout` | int | 5000 | wait 타임아웃 (ms) |
| `--extract` | string | 없음 | 셀렉터 텍스트 추출 |
| `--all` | bool | false | `--extract`에서 모든 매치 |
| `--headers` | bool | false | 응답 헤더를 stderr에 출력 |
| `--timeout` | int | 30 | 전체 타임아웃 (초) |

### 8.3 실행 순서

```
1. URL fetch → Page 로드
2. --wait     → 셀렉터 대기 (옵션)
3. --fill     → 입력 채우기 (옵션)
4. --click    → 요소 클릭 (옵션)
5. --press    → 키 입력 (옵션)
6. --eval     → JS 평가 (옵션)
7. 출력:
   --extract 있으면 → 셀렉터 매치 텍스트
   --summary 있으면 → 페이지 메타데이터
   그 외            → --format 형식의 콘텐츠
```

### 8.4 예시

```bash
# 페이지 읽기
oxibrowser fetch https://example.com --format markdown --json

# 버튼 클릭 후 결과 읽기
oxibrowser fetch https://example.com \
  --click "button#load" \
  --wait ".results" \
  --format markdown \
  --json

# JS 실행 결과만
oxibrowser fetch https://example.com --eval "document.title" --json

# 특정 요소 텍스트 추출
oxibrowser fetch https://example.com --extract "h1" --json

# 페이지 메타데이터만
oxibrowser fetch https://example.com --summary --json
```

---

## 9. `extract` 명령 — 전체 사양

### 9.1 사용법

```bash
oxibrowser extract <url> [flags]
```

### 9.2 플래그

| 플래그 | 타입 | 기본값 | 설명 |
|-------|------|--------|------|
| `--selector` | string | 없음 | CSS selector |
| `--all` | bool | false | 모든 매치 반환 |
| `--attrs` | string | `text` | 추출할 속성 (쉼표 구분) |
| `--links` | bool | false | 모든 `<a href>` 추출 |
| `--title` | bool | false | `<title>` 텍스트 |
| `--text` | bool | false | `<body>` 전체 텍스트 |
| `--markdown` | bool | false | 페이지 마크다운 |
| `--max-bytes` | int | 없음 | 출력 바이트 제한 |
| `--json` | bool | false | JSON으로 출력 |
| `--timeout` | int | 30 | 타임아웃 (초) |

### 9.3 `--attrs` 동작

```bash
# 기본 (text만)
oxibrowser extract <url> --selector "h2" --all --json
# → {"items":["Heading 1","Heading 2"]}

# 특정 속성
oxibrowser extract <url> --selector "a" --all --attrs text,href --json
# → {"items":[{"text":"Rust","href":"https://rust-lang.org"},...]}

# data 속성
oxibrowser extract <url> --selector "div.item" --attrs text,data-id --json
# → {"items":[{"text":"Item 1","data-id":"42"}]}
```

### 9.4 플래그 없으면 기본 동작

```bash
# 아무 플래그도 없으면 title + body 텍스트
oxibrowser extract <url> --json
```

```json
{
  "ok": true,
  "data": {
    "title": "Example Domain",
    "text": "This domain is for use in illustrative examples..."
  }
}
```

---

## 10. `skill` 명령 — 에이전트 가이드

```bash
oxibrowser skill
```

출력 (stdout, 마크다운):

```markdown
# OxiBrowser Agent Skills

## 3 Modes

1. **One-shot**: `oxibrowser fetch <url> [flags]` or `extract <url> [flags]`
2. **Automation**: `oxibrowser run <script.yaml>`
3. **Interactive**: `oxibrowser session --json` (stdin commands, stdout JSON)

## Invariant Rules

- ALWAYS add `--json` for machine-readable output
- ALWAYS add `--max-bytes 8000` to limit response size
- Use `--summary` first to check if a page is relevant before full read
- Use `--fields url,title,status` to skip large content fields
- Use `session` for multi-step interactions (click → read → click → read)

## One-shot Patterns

- Read page: `oxibrowser fetch <url> --format markdown --json --max-bytes 8000`
- Get links: `oxibrowser extract <url> --links --json`
- Extract elements: `oxibrowser extract <url> --selector "h2" --all --json`
- Click and read: `oxibrowser fetch <url> --click <sel> --wait <sel> --format markdown --json`
- Page metadata: `oxibrowser fetch <url> --summary --json`
- Run JS: `oxibrowser fetch <url> --eval "document.title" --json`

## Session Workflow

1. Start: `oxibrowser session --json` (run as subprocess)
2. Create tab: `new` → get tab_id
3. Navigate: `goto <tab_id> <url>`
4. Interact: `click/fill/press/eval <tab_id> ...`
5. Extract: `extract/content <tab_id> ... --max-bytes 8000`
6. Close: `close <tab_id>` then `exit`

## Output Format

All JSON output: `{"ok": true/false, "data": {...}, "error": "...", "error_code": "...", "meta": {"elapsed_ms": N}}`
Check `ok` first. If `false`, read `error_code`.

## Exit Codes

0=success, 1=runtime error, 2=input validation, 3=timeout, 4=network
```

~150 토큰. 에이전트 시스템 프롬프트에 직접 넣을 수 있다.

---

## 11. 기존 코드 재사용

변경이 필요 없는 것:

| 자산 | 재사용 |
|------|--------|
| `Tab` (`tab.rs`) | `session`이 `HashMap<String, Tab>`으로 관리 |
| `BrowseResult` | `--fields`로 필드 필터링만 추가 |
| `ScriptRunner` | `run` 명령에서 그대로 사용 |
| `BrowserConfig` | 모든 모드에서 동일 |
| `Browser::new_tab()` | `session`과 one-shot 모두 사용 |
| CDP 서버 (`serve`) | 변경 없음 |
| `fetch`/`extract` 로직 | one-shot 모드에서 그대로 |

실제 새로 짜야 할 것:

| 신규 | 내용 | 크기 |
|------|------|------|
| `session` REPL | 명령 파서 + 라우터 + stdin/stdout 루프 | ~400줄 |
| `validate` 모듈 | 입력 검증 함수들 | ~100줄 |
| `describe` 출력 | CLI 스키마 정적 JSON | ~150줄 |
| `skill` 출력 | 정적 마크다운 문자열 | ~50줄 |
| `--max-bytes` | 응답 잘라내기 유틸 | ~50줄 |
| `--fields` | 필드 필터링 유틸 | ~30줄 |
| `--attrs` | DOM 속성 추출 로직 | ~80줄 |
| `--summary` | 페이지 메타데이터 추출 | ~60줄 |
| 에러 JSON 래퍼 | 표준 응답 직렬화 | ~40줄 |

총 ~960줄 신규 코드. 기존 ~25,000줄은 변경 없음.

---

## 12. 마이그레이션

### Phase 1: CLI 개선 (1-2주)

기존 `fetch`, `extract`, `run`, `serve` 구조 유지. 플래그와 출력만 개선.

- [ ] `--max-bytes` 플래그 (모든 출력 명령)
- [ ] `--fields` 플래그 (fetch)
- [ ] `--attrs` 플래그 (extract)
- [ ] `--summary` 플래그 (fetch)
- [ ] 에러 출력 JSON 통일 (`{"ok":false,"error":"...","error_code":"..."}`)
- [ ] exit code 체계 (0/1/2/3/4)
- [ ] 입력 검증 모듈
- [ ] 자동 JSON 감지 (piped면 JSON)
- [ ] `describe` 서브커맨드
- [ ] `skill` 서브커맨드
- [ ] `browse` → `fetch`로 흡수 (`--click`, `--fill`, `--wait`, `--extract` 플래그)
- [ ] `eval <url>` → `fetch --eval`로 변경 (기존 `eval`은 deprecated)

### Phase 2: `session` (2-3주)

- [ ] 세션 명령 파서 (플레인 텍스트 → 세션 명령 구조체)
- [ ] 세션 명령 라우터 (명령 → Tab 메서드 호출)
- [ ] stdin/stdout JSON REPL 루프
- [ ] TabManager: `HashMap<String, Tab>` + ID 생성
- [ ] EOF/SIGTERM 정리
- [ ] `session` 내부 명령: `new`, `goto`, `back`, `forward`, `reload`
- [ ] `click`, `fill`, `press`, `type`, `select`, `check`, `scroll`
- [ ] `eval`, `extract`, `content`, `screenshot`, `wait`
- [ ] `close`, `close --all`, `list`, `help`, `exit`
- [ ] 세션 내부 명령에도 `--max-bytes`, `--fields`, `--attrs` 적용

### Phase 3: `run` 강화 + 폴리싱 (1주)

- [ ] `run` 결과 JSON을 표준 응답 래퍼로 통일
- [ ] `describe`에 `session` 내부 명령 스키마 추가
- [ ] `skill` 내용 검증 (실제 에이전트로 테스트)
- [ ] 통합 테스트: one-shot, session, run 3가지 모드

### Phase 4: MCP (필요시, 1-2주)

- [ ] `session`의 stdin/stdout을 MCP 프로토콜로 래핑
- [ ] `oxibrowser serve --stdio` (MCP 모드)
- [ ] 또는 별도 `oxibrowser mcp` 명령

---

## 13. 전후 비교

### Before (CLI 1.0)

```bash
# 에이전트가 "Hacker News 댓글 읽고 싶어"

oxibrowser browse "https://news.ycombinator.com/item?id=123" \
  --format markdown --extract ".comment" --all --json
# → 한 번에 전부, 100개 댓글이면 토큰 초과
```

### After (CLI 2.0)

```bash
# 옵션 1: one-shot (간단)
oxibrowser fetch "https://news.ycombinator.com/item?id=123" \
  --format markdown --max-bytes 8000 --json
# → 8KB만, truncated=true면 더 읽을지 결정

# 옵션 2: session (상호작용)
# → oxibrowser session --json (subprocess)
# → new
# ← tab_id
# → goto t1 https://news.ycombinator.com/item?id=123
# → content t1 --summary --json
# ← {"links_count":42,"text_length":12345}
# → "댓글만 추출하자"
# → extract t1 --selector ".comment" --all --max-bytes 8000 --json
# ← {"items":[...],"truncated":true,"total_bytes":45678}
# → "더 읽자"
# → extract t1 --selector ".comment" --all --max-bytes 16000 --json
# ← {"items":[...],"truncated":false}
# → close t1
# → exit

# 옵션 3: run (자동화)
oxibrowser run hn-extract.yaml --var post_id=123
```

에이전트가 **토큰 예산을 직접 제어**한다. 세 가지 진입점으로 모든 상황 커버.
