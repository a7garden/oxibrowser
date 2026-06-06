# Proposal: `oxibrowser search` — 통합 웹 검색 명령어

## 요약

oxibrowser가 브라우저인 만큼, 웹 검색 기능을 내장한다. 별도 도구(a3s-search) 없이 단일 바이너리로 모든 검색을 처리한다.

## 설계 원칙

1. **API 키 불필요** — 설치 즉시 검색 가능해야 한다 (GitHub는 선택적 토큰)
2. **단일 소스 실패 → fallback** — Bing이 CAPTCHA 걸리면 DuckDuckGo로 자동 전환
3. **JSON 일관성** — 모든 소스가 동일한 출력 스키마 사용
4. **브라우저 렌더링 불필요** — HTTP 요청 한 번으로 끝나는 경량 검색 (풀 페이지 로딩 안 함)

## CLI 인터페이스

```bash
oxibrowser search <query>
  --source <web|github|github-issues|github-wiki>   (기본: web)
  --engine <ddg|wiki|bing>                           (web 전용, 기본: ddg)
  --repo <owner/repo>                                 (github-xxx 전용)
  --token <ghp_xxx>                                   (github 전용, 선택 사항)
  --json                                              (기계 판독용)
  --max-results <N>                                   (기본: 10, 최대: 30)
  --timeout <sec>                                     (기본: 15)
```

### 사용 예시

```bash
# 웹 검색 (DuckDuckGo 기본)
oxibrowser search "rust programming" --json

# Wikipedia
oxibrowser search "tokio" --engine wiki --json

# Bing (CAPTCHA 걸리면 자동 DuckDuckGo fallback)
oxibrowser search "latest ai news" --engine bing --json

# 여러 엔진 동시 검색 (결과 병합)
oxibrowser search "oxibrowser" --engine ddg,wiki,bing --json

# GitHub 리포지토리 검색
oxibrowser search "headless browser rust" --source github --json

# GitHub 특정 리포의 이슈 검색
oxibrowser search "memory leak" --source github-issues --repo a7garden/oxibrowser --json

# GitHub 토큰으로 rate limit 해제
oxibrowser search "oxibrowser" --source github --token ghp_xxxx --json
```

## 출력 스키마

모든 소스가 다음 JSON 스키마로 통일된다:

```json
{
  "ok": true,
  "meta": {
    "elapsed_ms": 342,
    "source": "web",
    "engine": "ddg"
  },
  "data": {
    "query": "rust programming",
    "total_results": 42,
    "results": [
      {
        "title": "The Rust Programming Language",
        "url": "https://www.rust-lang.org/",
        "snippet": "A language empowering everyone...",
        "source": "DuckDuckGo"
      }
    ]
  }
}
```

GitHub 소스는 추가 필드를 포함한다:

```json
{
  "results": [
    {
      "title": "a7garden/oxibrowser",
      "url": "https://github.com/a7garden/oxibrowser",
      "snippet": "Headless browser in pure Rust...",
      "source": "GitHub",
      "extra": {
        "stars": 128,
        "forks": 12,
        "language": "Rust",
        "topics": ["browser", "headless", "cdp"],
        "updated_at": "2026-06-05T12:12:41Z"
      }
    }
  ]
}
```

## 소스별 구현

### 1. DuckDuckGo (`--engine ddg`, 기본값)

**엔드포인트**: `GET https://lite.duckduckgo.com/lite/?q=<query>&kl=us-en`

- 순수 HTML 응답 (JS 불필요)
- `<tr class="result">` 블록 파싱
- 결과 없을 시 Instant Answer API 폴백: `GET https://api.duckduckgo.com/?q=<query>&format=json&no_html=1`
- **장점**: CAPTCHA 없음, API 키 불필요, 빠름
- **구현**: `reqwest` GET + html5ever 또는 단순 정규식/문자열 파싱

### 2. Wikipedia (`--engine wiki`)

**엔드포인트**: `GET https://en.wikipedia.org/w/api.php?action=opensearch&search=<query>&limit=10&format=json`

- OpenSearch API는 JSON 배열 반환: `["query", ["title1", ...], ["url1", ...], ["snippet1", ...]]`
- **장점**: 공식 API, 안정적, CAPTCHA 없음
- **구현**: `reqwest` GET + `serde_json` 파싱 (20줄)

### 3. Bing (`--engine bing`)

**엔드포인트**: `GET https://www.bing.com/search?q=<query>`

- HTML 스크래핑, `<li class="b_algo">` 블록 파싱
- CAPTCHA 발생 시 → DuckDuckGo 자동 fallback (사용자에게 로그 기록)
- **구현**: `reqwest` GET + HTML 파싱 + CAPTCHA 감지

**CAPTCHA 감지 로직**:
```rust
if response_body.contains("CaptchaChallenge") || response_body.contains(" Bing captcha ") {
    tracing::warn!("Bing CAPTCHA challenge detected, falling back to DuckDuckGo");
    return fallback_to_duckduckgo(query).await;
}
```

### 4. GitHub 리포지토리 (`--source github`)

**엔드포인트**: `GET https://api.github.com/search/repositories?q=<query>&sort=stars&per_page=<N>`

- 표준 REST API, JSON 응답
- 인증 없이 60 req/hr, 토큰(`--token`) 있으면 5000 req/hr
- `User-Agent` 헤더 필수
- **구현**: `reqwest` GET + `serde_json` 파싱

### 5. GitHub 이슈 (`--source github-issues`)

**엔드포인트**: `GET https://api.github.com/search/issues?q=<query>+repo:<owner>/<repo>&sort=updated&per_page=<N>`

- `--repo <owner/repo>` 필수
- Pull Request도 포함됨 (PR 제외하려면 `+type:issue` 추가)
- **구현**: `reqwest` GET + `serde_json`

### 6. GitHub 위키 (`--source github-wiki`)

**엔드포인트**: 없음 → **별도 수단**

GitHub는 Wiki 전용 검색 API를 제공하지 않는다. 대안:

- **옵션 A**: GitHub Code Search API:`GET https://api.github.com/search/code?q=<query>+repo:<owner>/<repo>.wiki`
- **옵션 B**: `git clone --depth 1 <repo>.wiki.git` 후 로컬 grep

**권장**: 옵션 B가 간단하고 안정적. oxibrowser의 `run` 명령어로 git clone 후 grep하는 식으로 처리 가능하므로, `github-wiki`는 별도 명령어로 빼거나 **v1에서 제외**.

## 구현 계획

### Phase 1: 웹 검색 (web)

| 엔진 | 우선순위 | 이유 |
|------|---------|------|
| DuckDuckGo | P0 | 기본값, 가장 안정적 |
| Wikipedia | P0 | 공식 API, 구현 쉬움 |
| Bing | P1 | CAPTCHA 위험 있지만 사용자 요청 |

### Phase 2: GitHub 검색

| 소스 | 우선순위 | 이유 |
|------|---------|------|
| repositories | P0 | 검색 API 깔끔함 |
| issues | P0 | `--repo` 플래그만 추가 |
| wikis | P2 | 별도 접근 방식 필요 |

### Phase 3: 다중 엔진 병합

```
oxibrowser search "query" --engine ddg,bing,github --json
```

→ 모든 소스에서 병렬 수집 → 결과 병합 → 중복 제거 → JSON 반환

## 코드 구조

```
crates/oxibrowser/src/
├── search/
│   ├── mod.rs          # search 명령어 entry + 라우팅
│   ├── engine.rs       # SearchEngine trait
│   ├── ddg.rs          # DuckDuckGo 구현
│   ├── wiki.rs         # Wikipedia 구현
│   ├── bing.rs         # Bing 구현 (fallback 로직 포함)
│   └── github.rs       # GitHub repo/issues 구현
├── main.rs             # clap에 search subcommand 추가
```

### `SearchEngine` 트레잇

```rust
#[async_trait]
trait SearchEngine: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchItem>, SearchError>;
}
```

각 엔진이 이 트레잇을 구현하고, `search` 명령어는 등록된 엔진 목록을 순회한다.

## 기존 명령어와의 관계

```
oxibrowser
├── fetch       ← 페이지 콘텐츠 획득 (렌더링 O)
├── extract     ← 페이지 데이터 추출 (렌더링 O)
├── search      ← NEW: 검색 엔진 질의 (렌더링 X, HTTP only)
├── run         ← YAML 스크립트 실행
├── session     ← 인터랙티브 세션
├── serve       ← CDP 서버
├── describe    ← 스키마 출력
└── skill       ← 가이드 출력
```

`search`는 렌더링이 필요 없으므로 `fetch`보다 가볍다. 검색 결과 URL은 이후 `fetch`나 `extract`로 이어질 수 있다.

## pi-oxibrowser 확장과의 관계

파이 확장에서는 기존 3개 브라우징 툴(`browse`, `browse_extract`, `browse_script`) + 2개 검색 툴(`web_search`, `get_search_results`)을 유지하되, 내부 구현을 `oxibrowser search`로 통일:

```typescript
// 현재: a3s-search CLI 호출
const raw = await a3sSearch(pi, [query, ...]);

// 변경: oxibrowser search CLI 호출
const result = await oxibrowser(pi, ["search", query, "--json"]);
```

GitHub 검색이 필요하면 별도 툴(`github_search`)을 추가하거나 `web_search`의 `source` 파라미터로 처리.

## 고려사항

### CAPTCHA 처리
- Bing만 CAPTCHA 위험 있음
- 감지 시 DuckDuckGo로 자동 fallback, 사용자에게 `tracing::warn!` 로 로깅
- 사용자 대화형(`--interactive`) 모드에서는 CAPTCHA 이미지 URL 출력?

### Rate Limit
- GitHub: 인증 없음 60/hr, 토큰 있음 5000/hr
- DuckDuckGo: 별도 문서화된 제한 없음 (과도한 요청 주의)
- Wikipedia: 합리적인 사용 범위 내 제한 없음

### User-Agent 헤더
GitHub API는 `User-Agent` 필수. `oxibrowser/<version>` 사용.
```rust
.header("User-Agent", &format!("oxibrowser/{}", env!("CARGO_PKG_VERSION")))
```

### 캐싱 (향후)
동일 쿼리 반복 시 디스크 캐시 고려. Phase 3 이후 검토.
