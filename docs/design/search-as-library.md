# 설계: Search 모듈을 라이브러리 API로 공개

> **상태**: 구현 완료
> **동기**: oxi(Rust 코딩 에이전트)가 oxibrowser를 이미 의존성으로 사용 중. search 기능도 같은 crate에서 호출하고 싶음.

---

## 왜 별도 crate가 필요 없는가

oxi는 이미 oxibrowser의 브라우징 기능(fetch, extract, run, session 등)을 쓴다.
따라서 oxibrowser-core, oxibrowser-cdp, clap 등은 이미 의존성에 있다.

search만 따로 빼서 "가벼운 의존성"을 만들 이유가 없다.
**이미 다 컴파일하고 있는 crate에서 search 모듈 하나를 공개하면 끝.**

## 무엇을 했나

### 변경 1: `lib.rs`에 search 공개

```rust
// crates/oxibrowser/src/lib.rs
pub mod search;

pub use oxibrowser_core::error::Result;
pub use oxibrowser_core::Browser;
pub use oxibrowser_core::BrowserConfig;

// 편의 re-export
pub use search::engine::{GitHubExtra, SearchEngine, SearchError, SearchOutput, SearchResult};
```

한 줄(`pub mod search`)이 핵심. 나머지 re-export는 사용자가 `oxibrowser::SearchResult`처럼 바로 접근할 수 있게 하는 편의 기능.

### 변경 2: `main.rs`에서 search 모듈 참조 방식 변경

```rust
// Before: main.rs가 자체 mod tree에 search를 선언
mod search;

// After: lib.rs에 선언된 pub mod를 가져옴
use oxibrowser::search;
```

같은 소스코드, 같은 타입. 다만 이제 `lib.rs`가 모듈의 소유권을 갖고, `main.rs`는 그것을 빌려 쓴다.

### 변경 3: `search/mod.rs` 내부 아이템 공개

`WebEngine`, `parse_engines()` 등을 `pub`으로 변경.
dispatch()만으로 충분하지만, 커스텀 엔진 조합이 필요할 때 직접 제어 가능.

## oxi에서의 사용

```rust
// oxi-agent/Cargo.toml — 이미 oxibrowser 의존성 있음. 변경 없음.

// oxi-agent/src/tools/web_search.rs
use oxibrowser::search;

let output = search::dispatch(
    query, "web", "ddg,wiki,bing",
    None, None, 10, 15,
).await?;

// 편의 re-export로 바로 타입 사용도 가능
use oxibrowser::{SearchResult, SearchOutput};
```

## 의존성 그래프 (변경 없음)

```
oxi → oxibrowser → oxibrowser-search (별도 crate 아님, 그냥 pub mod)
                  → oxibrowser-core
                  → oxibrowser-cdp
```

oxi는 이미 전부 가지고 있으니, 새 의존성 추가 없이 search 기능 확보.

## 파일 변경 요약

| 파일 | 변경 |
|------|------|
| `lib.rs` | `pub mod search` + re-export 추가 |
| `main.rs` | `mod search` → `use oxibrowser::search` |
| `search/mod.rs` | `WebEngine`, `parse_engines`을 pub으로 |

테스트: 86개 전부 통과. 로직 변경 없음.
