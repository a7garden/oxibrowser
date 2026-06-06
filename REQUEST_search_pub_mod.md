# 요청: search 모듈을 pub으로 공개해주세요

## 배경

oxi(https://github.com/earendil-works/pi의 Rust 포트)는 현재 웹 검색을 위해 `a3s-search` 크레이트(v1.2.3)를 사용하고 있습니다. oxibrowser 0.14.0에 search 기능이 추가된 것을 확인했고, oxi의 검색 의존성을 `a3s-search`에서 oxibrowser로 통합하고 싶습니다.

## 현재 상황

### oxibrowser 0.14.0의 search (CLI 서브명령어)

```
oxibrowser search "query" --engine ddg,wiki,bing --json
oxibrowser search "query" --source github --json
oxibrowser search "query" --source github-issues --repo owner/repo --json
```

- 엔진: ddg, wiki, bing (+ GitHub repos/issues)
- 멀티엔진 병렬 실행 + URL 기반 중복 제거
- JSON 출력 지원

### oxi의 a3s-search 사용 (라이브러리)

```rust
// oxi-agent/src/tools/web_search.rs
let mut search = a3s_search::Search::new();
search.add_engine(a3s_search::engines::DuckDuckGo::new());
let results = search.search(query).await?;
```

- 엔진: ddg, wiki, bing, brave
- 러스트 라이브러리로 직접 호출 (subprocess 아님)
- SearchResult 캐싱 + searchId 기반 get_search_results 도구

## 문제점

oxibrowser의 search가 `lib.rs`에 공개되지 않아서, oxibrowser를 라이브러리 의존성으로 사용하는 입장에서 search 기능을 직접 호출할 수 없습니다. 현재 `lib.rs`는 다음만 재-export합니다:

```rust
// crates/oxibrowser/src/lib.rs
pub use oxibrowser_core::error::Result;
pub use oxibrowser_core::Browser;
pub use oxibrowser_core::BrowserConfig;
```

search 모듈 5개(`ddg`, `wiki`, `bing`, `github`, `engine`)와 `dispatch()` 함수가 CLI(`main.rs`)에서만 사용 가능합니다.

## 요청 사항

`search` 모듈을 라이브러리 퍼블릭 API로 공개해주세요. 구체적으로:

### 1. `lib.rs`에서 search 모듈 공개

```rust
// crates/oxibrowser/src/lib.rs
pub mod search;  // 추가

pub use oxibrowser_core::error::Result;
pub use oxibrowser_core::Browser;
pub use oxibrowser_core::BrowserConfig;

// 편의 재-export
pub use search::engine::{SearchEngine, SearchError, SearchResult, SearchOutput, GitHubExtra};
```

### 2. search 내부 모듈의 공개 범위 조정

현재 `search/mod.rs`의 `WebEngine` enum, `parse_engines()`, `search_web()` 등이 private(`fn`)입니다. 다음 최소한의 공개 API만 필요합니다:

| 항목 | 가시성 변경 | 비고 |
|------|-------------|------|
| `search::dispatch()` | 이미 `pub async fn` | ✅ 변경 불필요 |
| `search::engine::SearchResult` | 이미 `pub struct` | ✅ 변경 불필요 |
| `search::engine::SearchOutput` | 이미 `pub struct` | ✅ 변경 불필요 |
| `search::engine::SearchError` | 이미 `pub enum` | ✅ 변경 불필요 |
| `search::engine::SearchEngine` | 이미 `pub trait` | ✅ 변경 불필요 |
| `search::engine::GitHubExtra` | 이미 `pub struct` | ✅ 변경 불필요 |
| `search::engine::build_search_client()` | 이미 `pub fn` | ✅ 변경 불필요 |
| `search::format_human()` | 이미 `pub fn` | ✅ 변경 불필요 (CLI 전용이지만 oxi에서 안 씀) |

**핵심: `pub mod search` 한 줄이면 대부분 끝납니다.** 타입들이 이미 `pub`으로 선언되어 있습니다.

### 3. `search/mod.rs` 내부 아이템 공개 (선택, 부가적)

oxi에서 커스텀 엔진을 만들거나 직접 엔진을 제어하려면 다음도 공개하면 좋습니다:

```rust
// search/mod.rs
pub enum WebEngine { ... }        // 현재 private
pub fn parse_engines(...) { ... } // 현재 private (fn)
```

하지만 oxi의 사용 패턴에서는 `dispatch()` 함수만으로 충분합니다. 이 항목은 Nice-to-have입니다.

## oxi에서의 사용 예상

```rust
// oxi-agent/Cargo.toml
oxibrowser = { version = "0.15", default-features = false }  // CLI 없이 lib만

// oxi-agent/src/tools/web_search.rs
use oxibrowser::search;

async fn do_search(&self, query: &str, engines: &str, limit: usize) -> Result<...> {
    let output = search::dispatch(
        query,
        "web",           // source
        engines,         // "ddg,wiki,bing"
        None,            // repo
        None,            // token
        limit,
        15,              // timeout_secs
    ).await?;

    // output.results → Vec<SearchResult>
    // SearchCache에 저장 후 searchId 리턴
}
```

## a3s-search 제거 시 이점

| 항목 | 효과 |
|------|------|
| 의존성 통합 | oxi는 이미 oxibrowser-core에 의존 → search도 같은 생태계 |
| GitHub 검색 | oxibrowser에만 있는 기능 (repos, issues)을 oxi에서도 사용 가능 |
| 유지보수 | 검색 엔진 로직을 oxibrowser에서 단일 관리 |
| a3s-search 제거 | oxi-agent의 외부 의존성 1개 감소 |

## 호환성 참고

- `default-features = false`로 사용 시 CLI 의존성(clap, tracing-subscriber)은 빠져야 합니다. 현재 `oxibrowser` 크레이트의 `[dependencies]`에 clap이 포함되어 있어, 이 부분은 feature flag로 분리하거나 별도 `oxibrowser-search` 크레이트를 고려할 수도 있습니다.
- 대안: search 모듈이 실제로 필요한 의존성은 `reqwest`, `serde`, `serde_json`, `url`, `async-trait`, `futures`, `tokio`뿐입니다. clap, tracing-subscriber, oxibrowser-cdp 등은 search에서 사용하지 않습니다.

## 우선순위

1. **최소 요청**: `pub mod search` 한 줄 추가 → oxi에서 `search::dispatch()` 사용 가능
2. **권장**: feature flag 분리 (`search` feature)로 불필요한 의존성(clap 등) 제거
3. **이상적**: `oxibrowser-search` 별도 크레이트로 분리 (reqwest/serde만 필요한 경량 크레이트)

감사합니다!
