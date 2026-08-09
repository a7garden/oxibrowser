# Phase 6 — Network Correctness (spec)

> **작성:** 2026-08-09 · **분기:** `main`
> **상위:** `docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md` (§3 Phase 6),
> `docs/superpowers/specs/2026-08-09-post-phase5-remaining-roadmap.md` (§4)
> **하드 제약:** pure Rust 유지. `wreq`+`btls` 전송 계층은 완성(Phase 3) — 이 phase는
> 그 위의 **정책 계층**을 채운다.
>
> **검증 바(상위 로드맵 §6):** 각 작업은 main에서 실패하는 테스트 → 통과로 마감.
> CORS/쿠키/Referer/auth는 `wiremock`(워크스페이스에 이미 있음) 기반 통합 테스트로
> real round-trip 검증.

---

## 0. 현재 상태 (한 줄)

전송(HTTP/1.1+H2+TLS+gzip/br)은 완성. 정책 계층(CORS, 쿠키 만료/PSL/CHIPS, proxy,
auth, Referer)이 비어 있다. `HttpClient`(`client.rs`)는 `wreq::Client`를 감싸고 SSRF
필터 + 쿠키 첨부/저장만 수행.

앵커 디렉토리: `crates/oxibrowser-core/src/network/`
(`client.rs`, `cookie.rs`, `ip_filter.rs`, `intercept.rs`, `resource.rs`, `ws.rs`,
`robots.rs`).

---

## 1. 작업 분해 (구현 순서 = 의존성 순)

cookie 관련 작업은 서로 의존하므로 한 블록으로 묶어 순차 구현한다. CORS/Referer/auth/
proxy는 서로 독립적이나 모두 `HttpClient`를 통한다.

### 1.1 Cookie expiry / Max-Age (`cookie.rs`) — 토대

- **문제:** `CookieEntry`에 만료 정보가 없다. `parse()`가 `Expires`/`Max-Age` 속성을
  무시. 세션 쿠키와 영구 쿠키를 구분하지 못하고, 만료된 쿠키가 영원히 발송된다.
- **변경:**
  - `CookieEntry`에 `expires: Option<i64>`(Unix epoch 초), `max_age: Option<i64>`(초),
    `creation_time: Option<i64>` 추가. 직렬화 호환 유지(`#[serde(default)]`).
  - `parse()`가 `Expires=`(HTTP-date, `httpdate` 크레이트 또는 수동 파싱)와
    `Max-Age=`(정수 초)를 파싱.
  - `store()`: `Max-Age <= 0`이면 즉시 삭제(덮어쓰기 후 제거). 만료 시각 계산 저장.
  - `cookies_for_url*()`: 만료된 쿠키는 발송에서 제외(게으른 제거).
  - 게으른 정리: 조회 시 만료된 항목 skip + 배경 제거.
- **테스트:** 만료된 쿠키 미발송, `Max-Age=0` 즉시 삭제, 세션 쿠키(만료 없음) 유지,
  직렬화 라운드트립.
- **앵커:** `cookie.rs:46-109`(`CookieEntry`, `parse`), `:215-503`(jar).

### 1.2 Public Suffix List (`cookie.rs`) — 쿠키 스코프 정확도

- **문제:** `domain_matches`가 순수 접미사 매칭만 한다. `Domain=.co.uk` 같은 공개
  접미사에 대한 쿠키 설정이 허용될 수 있다(스코프 확장 공격).
- **변경:** `psl = "2.1.223"` 크레이트(Mozilla PSL 번들) 추가. `store()`에서
  `Domain=` 속성이 공개 접미사 자체와 정확히 일치하면 거부. eTLD+1 경계 확인.
- **테스트:** `Domain=.co.uk` 거부, `Domain=.example.com` 허용, IP 호스트는 PSL
  우회(기존 동작 유지).
- **앵커:** `cookie.rs:136-164`(`domain_matches`), `:260-278`(`store` 도메인 검증).

### 1.3 `__Host-` / `__Secure-` prefix (`cookie.rs`)

- **문제:** 쿠키 이름 prefix 검증이 없다.
- **변경:** `store()`에서:
  - `__Secure-`: `Secure` 속성 필수, 아니면 거부.
  - `__Host-`: `Secure` + `Path=/` + `Domain` 없음(호스트 전용) 필수, 아니면 거부.
- **테스트:** 각 prefix의 유효/무효 조합.
- **앵커:** `cookie.rs:store()`(`:235-317`).

### 1.4 CHIPS 분할 쿠키 (`cookie.rs`, `client.rs`)

- **문제:** `Partitioned` 속성 미지원. 타사 쿠키 분할 없음.
- **변경:**
  - `CookieEntry`에 `partitioned: bool` 추가 + `parse()`에서 인식.
  - 저장 키를 `(registrable_domain, partition_key)` 튜플로 확장. 분할 쿠키는
    최상위 프레임의 eTLD+1 파티션 키와 함께 저장/조회.
  - 토대 구현: 파티션 키 추적은 이번 phase에서 파티션된 저장/조회 경로만 열어두고,
    상위 프레임 도메인 전파는 Phase 8(iframe)과 연계.
- **테스트:** `Partitioned` 속성 파싱, 분할 쿠키가 다른 파티션에서 보이지 않음.
- **앵커:** `cookie.rs:store/cookies_for_url*`.

### 1.5 CORS + preflight (`client.rs`, JS fetch 경로)

- **문제:** `Origin` 헤더 미송출, `Access-Control-*` 응답 헤더 미해석, 사전 OPTIONS
  프리플라이트 없음. 크로스오리진 fetch가 브라우저 정책 없이 전송된다.
- **변경:**
  - 요청에 `Origin: <현재 페이지 origin>` 자동 송출(JS fetch/XHR 경로에서 페이지
    origin 알 수 있을 때).
  - 단순 요청이 아닌 경우(사전 플라이트 필요 메서드/헤더) `OPTIONS` 프리플라이트
    전송 → `Access-Control-Allow-Origin`/`-Methods`/`-Headers` 해석 → 허용 시 본
    요청, 거부 시 네트워크 에러(TypedError `NetworkError`).
  - `Access-Control-Allow-Credentials` + 쿠키 상호작용.
- **테스트(wiremock):** 단순 크로스오리진 허용, 프리플라이트 후 본요청, 거부 시
  에러, 와일드카드 vs 구체적 origin.
- **앵커:** `client.rs:request()`(`:205-253`), JS fetch 네이티브
  (`runtime.rs` fetch 핸들러).

### 1.6 자동 Referer (`client.rs`)

- **문제:** `Referer` 헤더를 전혀 송출하지 않는다.
- **변경:** 기본 `ReferrerPolicy: strict-origin-when-cross-origin`(Chrome 기본).
  현재 페이지 URL 기반:
  - 동일 origin: 전체 URL(쿼리 제외 옵션).
  - 크로스 origin, 동일 프로토콜: origin만.
  - 다운그레이드(https→http): Referer 생략.
  - `Referrer-Policy` 응답 헤더 또는 `<meta name="referrer">` 존중(최소: 응답 헤더).
- **테스트(wiremock):** 동일/크로스 origin Referer, 다운그레이드 생략.
- **앵커:** `client.rs:fetch/request/intercept`.

### 1.7 Basic/Digest auth (`client.rs`, config)

- **문제:** `Authorization` 헤더 미지원, 401 챌린지 미해석.
- **변경:**
  - `BrowserConfig`에 credentials(username/password) 또는 자동 챌린지 응답.
  - 401 + `WWW-Authenticate: Basic` → 재시도 with `Authorization: Basic <base64>`.
  - Digest: `WWW-Authenticate: Digest` 파싱 → nonce/realm 해시 재계산 → `Authorization:
    Digest ...`(MD5, RFC 7616 최소 subset).
- **테스트(wiremock):** basic 401→200, digest 401→200, 잘못된 자격증명 거부.
- **앵커:** `client.rs:fetch_with_challenge_retry` 패턴 재사용, `config.rs`.

### 1.8 HTTP/SOCKS proxy (`client.rs`, config)

- **문제:** proxy 설정 불가.
- **변경:** `BrowserConfig`에 `proxy: Option<String>` 추가. `Client::builder()`에
  `.proxy(wreq::Proxy::all(...))` 적용(HTTP/HTTPS/SOCKS5). CLI `--proxy` 플래그.
- **테스트:** 설정 파싱, 빌더 적용(실제 프록시 연결은 통합 테스트로 어려우면 단위
  테스트로 설정 검증).
- **앵커:** `client.rs:new()`(`:107-146`), `config.rs`, CLI(`main.rs`).

### 1.9 스트리밍 본문 (`client.rs`)

- **문제:** 본문을 전체 버퍼링한다(`read_body_limited`의 TODO 참조). wreq 6.0.0-rc가
  `bytes_stream()`/`chunk()`를 노출하지 않아 현재 불가.
- **방향:** wreq API 진행 상황 확인 → 노출 시 `http_body_util::BodyExt::frame` 기반
  스트리밍으로 전환. 의존성 한계로 차단되어 있으면 명시하고 후순위.
- **비고:** 의존성 게이트. 이 phase에서 wreq가 스트리밍을 지원하지 않으면 스킵 + 문서화.

---

## 2. 의존성 추가

- `psl = "2.1.223"` — Public Suffix List (cookie scope, §1.2).
- (선택) `httpdate` 또는 수동 HTTP-date 파싱 — `Expires` 속성(§1.1). `wreq`/`hyper`
  의존 트리에 이미 존재할 가능성; 확인 후 재사용.

---

## 3. 비목표 / 제외

- **의존성 한계 차단:** wreq 스트리밍 본문(§1.9)은 wreq API가 막혀 있으면 스킵.
- **파티션 키 상위 프레임 전파:** Phase 8(iframe) 연계. 이 phase는 저장/조회 경로만.
- **Referrer-Policy의 모든 세부 값:** Chrome 기본값 + 응답 헤더 존중. 모든 `<meta>`
  조합은 후순위.
- **Digest auth의 SHA-256/qop 전체:** RFC 7616 MD5 subset 우선.

---

## 4. 검증 게이트

```bash
cargo build --features browser --bin oxibrowser
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

각 작업 1 커밋, conventional-commit 메시지 (`feat(network): ...`).

---

끝.
