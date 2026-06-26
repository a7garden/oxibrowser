# OxiBrowser v0.16 제안서 — "실제 웹" 준비도 로드맵

> **논점**: v0.15까지 엔진은 탄탄하다. 하지만 "CDP 호환 AI 브라우저"라는 포지셔닝을
> **실제 서비스**에서 입증하려면 세 가지 장벽이 남았다. 이 문서는 그 장벽과 해법을
> 우선순위화해 제안한다.
>
> **기준일**: 2026-06-25 · **현재 버전**: 0.15.0 · **작성**: 코드 기반 검증 (검색/정적 분석)

---

## TL;DR

v0.15는 "데모는 멋지게 돌아간다"를 넘었다. 하지만 **"에이전트가 실제 인터넷을 쓴다"**에는
세 개의 실사용 장벽이 있다. 이 세 개를 막으면 `oxibrowser`는 동일 클래스의 순수-Rust
브라우저 중 유일하게 **"Cloudflare 보호 사이트 + SPA + 본문 추출"**을 동시에 만족하는 엔진이 된다.

| # | 장벽 | 한 줄 증상 | 영향 |
|---|------|-----------|------|
| 1 | **전송 핑거프린트** | HTTP/2 미지원 + rustls JA3 ≠ Chrome | Cloudflare/DataDome 급 사이트 차단 (실제 웹의 상당수) |
| 2 | **SPA 라우팅** | `history.pushState` / `location.href` 없음 | React/Vue/Svelte 앱이 첫 화면 로드 후 죽음 |
| 3 | **콘텐츠 추출 충실도** | 본문/구조화 데이터 추출이 최소 | 에이전트의 1차 가치("페이지 읽기") 약함 |

이 외에 **의미론적 레이아웃/관찰자 API/웹 컴포넌트**는 "더 많은 페이지가 동작"하게 하는
확장 영역이다.

---

## 1. 현재 상태 (검증 기반 — 중복 방지)

이 제안은 **이미 구현된 것을 다시 제안하지 않기 위해** 코드베이스를 직접 확인한 결과다.

### 이미 되어 있는 것 (v0.15)

| 기능 | 위치 | 비고 |
|---|---|---|
| `MutationObserver` | `js/runtime.rs:990` `notify_mutation_observers`, `:1690` 생성자, `:1776` `__moRegistry` | 실구현 — appendChild/removeChild 시 알림 |
| `getComputedStyle(el)` | `js/runtime.rs:5995` | 의미론적 충분 (display/color/`_visible`/`_interactive`) |
| `getBoundingClientRect()` | `js/runtime.rs:4636` | 클릭/드래그 좌표 계산에 사용 (tab.rs, mouse.rs) |
| 탭 계층 + per-tab 이벤트 | `BrowserEvent` (`:tab_id`) | v0.13 추가 |
| 관측가능성 (tracing) | `progress.md` 12/12 완료 | `#[tracing::instrument]` 전면 |
| **스텔스 Level-1 (JS 표면)** | `js/stealth.rs` (신규) | webdriver/plugins/chrome/WebGL/userAgentData/permissions. 본 세션에서 구현 |
| UA 전파 | `JsRuntimeConfig.user_agent` | 본 세션에서 사전 버그 수정 |

### 빠져 있는 것 (이 제안의 대상; 부재를 직접 검색으로 확인)

| 기능 | 확인 방법 | 결과 |
|---|---|---|
| History API (`pushState`/`history`) | `search` 전 코어 | **0 매치** — 부재 |
| `location.href` / `location.assign` | `search` 전 코어 | **0 매치** — 부재 |
| `IntersectionObserver` / `ResizeObserver` | `search` | **0 매치** |
| `customElements` / `attachShadow` (Shadow DOM) | `search` | **0 매치** |
| **HTTP/2** | `Cargo.toml:32` + `cargo tree -i h2` → "did not match any packages" | `http2` 피처 부재 → **`h2` 크레이트 자체가 트리에 없음** → HTTP/1.1 전용 |
| TLS 핑거프린트 | 상동 `rustls-tls` | rustls JA3/JA4 ≠ Chrome BoringSSL |

---

## 2. 제안 우선순위

> 형식은 기존 `roadmap-v0.5.md`와 동일: **What / Why / How / Files / Acceptance**.
> 노력은 S(며칠)·M(1-2주)·L(한 달 단위)로 표기.

### P0.1 — 전송 핑거프린트: HTTP/2 + Chrome-like TLS  (노력: L · 영향: 결정적)

**What**: (a) HTTP/2 지원을 켜고, (b) TLS ClientHello를 Chrome에 가깝게 모방한다.

**Why**: 현재 `reqwest`가 `default-features = false`로 빌드되어 **HTTP/2를 전혀 말하지 못한다.** (`cargo tree -i h2`로 확인: `h2` 크레이트가 의존성 트리에 **존재하지 않는다** — 단순히 기능이 꺼진 게 아니라 H2 구현 자체가 링크되지 않았다. H2-only 사이트는 접속 자체가 실패하고, 혼용 사이트에서는 HTTP/1.1 강제가 즉각적인 봇 신호다.)
또한 TLS 백엔드가 rustls이라 JA3/JA4가 Chrome(BoringSSL)과 다르다. 두 요인이 합쳐
Cloudflare/DataDome/Akamai 급 WAF가 "비정상 클라이언트"로 1차 차단한다.
이것이 **Level-1 스텔스(JS 표면)의 천장**이다 — 완벽한 JS-Chrome 표면 위에 비-Chrome TLS를
얹으면 역설적으로 **균일한 비-Chrome보다 더 강한 의심 신호**가 된다.

**How**:
1. **HTTP/2 켜기 (저비용)**: `reqwest` 피처에 `"http2"` 추가. rustls + ALPN `h2` 협상 확인.
   대부분의 최신 사이트가 H2-only이거나 H2를 선호하므로, 이것만으로도 호환성이 크게 오른다.
2. **TLS ClientHello 정합 (고비용·연구 집약)**: 초안의 "(A) boring / (B) rustls 트레이드오프"는 **둘 다 과대평가**임이 확인됨(2026-06-25 재검토):
   - **(B) rustls**: 공개 `ClientConfig`는 cipher suite·ALPN·지원 버전만 노출. ClientHello **확장 순서·GREASE 배치는 rustls 내부(`rustls::client::hs`)에 고정** → JA3/JA4 정합에는 **포크가 필수**. "근사치"도 불가.
   - **(A) `boring`**: BoringSSL *라이브러리* 링크 = Chrome과 동일 스택이지만, **기본 `SslConnector`가 Chrome ClientHello를 내보내지 않음**. Chrome 핑거프린트는 curl-impersonate가 BoringSSL 소스를 패치하는 이유처럼 **소스 수준 패치**가 필요.
   - 결론: 순수 트레이드오프가 아니라 **T2는 impersonation 크레이트/포크가 전제**인 연구 단위. 제안: **T1(v0.16) HTTP/2 연결성만** / **T2+T3(v0.17+) TLS 정합 + H2 프레임 정합을 하나의 엔지니어링 단위로**.
3. **HTTP/2 프레임 핑거프린트 (T2와 결합)**: SETTINGS 값/순서, pseudo-header 순서, WINDOW_UPDATE 캐던스는 **reqwest/hyper 공개 API로 제어 불가**. Chrome 정합 = reqwest 우회 후 `h2` 크레이트 직접 구동. "(A) 경로에서 자연스럽게 따라온다"는 비현실적.
4. **공급망 리스크(사전 확인 권장)**: Rust impersonation 도구는 Go `utls` 대비 미성숙. T2에 "L" 노력 배정 전, 현재 Chrome JA3를 실제 생성하는 **유지보수 중인 Rust 크레이트 존재 여부**를 먼저 확인 — 없으면 T2는 "통합"이 아니라 "포팅/구축".

**Files**: `Cargo.toml`(reqwest 피처), `network/client.rs`(TLS 커넥터 빌더 분기), 신규 `network/tls_impersonate.rs`

**Acceptance**:
- **(a) 수동 핑거프린트 티어 통과 (P0.1 범위)**: managed 챌린지가 *없는* Cloudflare/DataDome 타깃에서 200 (현재 403). 정합 효과를 특정 JA3 해시가 아닌 **행위 기반(통과율 %)**으로 측정 — 해시 고정은 Chrome 버전 롤링마다 유지보수 트레드밀.
- **(b) managed/JS 챌린지 해결은 P0.3의 범위**: `session.rs:281 navigate()`가 단일 fetch→decode→page이며 챌린지 감지/JS 실행/`cf_clearance` 재fetch 루프 없음(403은 `navigate_with_retry` 재시도 대상도 아님, `session.rs:394-395`). 이 루프는 P0.1이 아닌 **P0.3(WAF 챌린지 솔버)**이며, 성공은 boa가 챌린지 JS 환경 탐침을 통과하는지(P2.2)에 좌우 → **P0.1 ∩ P2.2 ∩ P0.3 동시 필요**.
- HTTP/2 응답 수신 로그 확인 (T1).

---

### P0.2 — SPA 라우팅: History API + `location`  (노력: M · 영향: 높음)

**What**: `history.pushState/replaceState/back/forward/go`, `window.location`(전체 접근자 + `assign/replace/reload`), `popstate` 이벤트.

**Why**: 현대 웹앱(React Router, Vue Router, SvelteKit)은 **클라이언트 사이드 라우팅**을 쓴다.
`history.pushState`가 없으면 첫 화면은 로드되지만 링크 클릭/네비게이션 직후 JS가 에러를 던지거나
무반응이 된다. "에이전트가 SPA를 탐색한다"는 핵심 시나리오가 깨진다.

**How**:
- `history` 객체: `length`/`state`/`scrollRestoration` 프로퍼티 + `pushState/replaceState/back/forward/go`.
  - `pushState(state, "", url)`: `Session.history` 벡터에 푸시, 현재 URL 갱신.
  - `back/forward/go`: `history_index` 이동 → `popstate` 이벤트 디스패치.
- `location`: `href/protocol/host/hostname/port/pathname/search/hash/origin` 접근자 + `assign/replace/reload`.
  - `location.href = url` setter, `assign(url)` → `Session::navigate(url)`.
- `popstate` 는 기존 `EventTarget`(`addEventListener`)에 올린다 — 디스패치 인프라는 이미 있음.

**Files**: `js/runtime.rs`(globals 섹션 — `history`/`location` 빌더), `session.rs`(history 필드는 이미 존재), `tab.rs`(navigate 호출)

**Acceptance**:
- `evaluate("history.pushState({},'','/p2'); history.length")` → 증가된 값.
- React Router 데모 사이트에서 링크 클릭 후 URL이 바뀌고 콘텐츠가 갱신됨.
- `evaluate("location.href")` 가 현재 URL 반환.

---
### P0.3 — WAF managed 챌린지 솔버 (Cloudflare/DataDome)  (노력: M · 영향: 결정적 for 챌린지 보호 사이트)

**What**: `session.rs:281 navigate()`의 단일 fetch→decode→page 파이프라인에 챌린지 감지→JS 실행→클리어런스 쿠키 획득→재fetch 루프를 추가. 403/503 챌린지 응답을 재시도 대상으로 승격(현재 `navigate_with_retry`는 status≥500만, `session.rs:394-395`).

**Why**: P0.1(수동 핑거프린트 티어)을 통과해도 managed 챌린지를 쓰는 사이트는 챌린지 인터스티셜을 내려주고 JS 실행 후 쿠키를 심는다. 이 티어는 전송 핑거프린트와 직교하며, **boa가 챌린지 JS의 환경 탐침을 통과해야(P2.2)만** 쿠키가 발급된다. 따라서 **P0.1 ∩ P2.2 ∩ P0.3 동시 충족** 시에만 managed 챌린지 보호 사이트가 200. 이 항목이 없으면 DoD #1의 "200"은 수동 티어 타깃으로만 성립한다.

**How**:
1. 챌린지 페이지 감지: Cloudflare(`/cdn-cgi/challenge-platform`)·DataDome·특정 응답 헤더/본문 지문으로 판별.
2. 챌린지 JS를 기존 boa 런타임에서 실행(엔진은 이미 있음) → `cf_clearance` 쿠키 캡처 → 동일 세션으로 재fetch.
3. `navigate_with_retry` 확장: 403 챌린지를 재시도 가능 상태로(루프 최대 횟수 제한).
4. 쿠키 체이닝: 기존 글로벌 쿠키 jar(`browser.rs:50`, `client.rs:99-107`) 재사용 — 인프라는 있음, 루프 로직이 빠져 있음.

**Files**: `session.rs`(navigate 챌린지 분기 + 재시도 정책), `network/client.rs`(챌린지 감지 헬퍼)

**Acceptance**:
- managed 챌린지를 사용하는 Cloudflare 타깃에서 `cf_clearance` 쿠키가 획득되고 200 도달.
- 챌린지 JS 실행 없이는 동일 타깃이 403으로 유지됨(회귀 가드).
- 단, 본 항목 성공은 **P2.2(boa 동일성) 선행**에 의존 — boa가 챌린지 탐침에 걸리면 쿠키 미발급.

---

### P1.1 — 콘텐츠 추출 충실도: 본문 + 구조화 데이터  (노력: M · 영향: 높음)

**What**: `OXI.getMarkdown`/`fetch --markdown`의 품질을 높이고, 구조화 데이터(JSON-LD, Open Graph, `schema.org`, 표/리스트) 추출을 추가한다.

**Why**: 에이전트가 브라우저를 쓰는 **1차 이유는 페이지 읽기**다. 현재 마크다운은 DOM 직변환에
가깝고(광고/네비/푸터가 본문에 섞임), 구조화 데이터 추출이 없다. 추출 품질 = 제품 가치.

**How**:
1. **본문 추출 (Readability 계열)**: 휴리스틱 본문 영역 탐지(텍스트 밀도·링크 비율·클래스명
   힌트). `getComputedStyle._visible`/`_interactive`를 활용해 숨김 요소를 사전 배제(이미 구현된
   의미론적 레이아웃과 시너지).
2. **구조화 데이터**: `<script type="application/ld+json">` 파싱 → `data.structured`;
   `<meta property="og:*">` → `data.opengraph`; `schema.org` Itemprop 수집.
3. **표/리스트 보존**: 마크다운 테이블·중첩 리스트 직렬화 개선.
4. **필드 선택**: 기존 `--fields` 확장 — `structured`, `opengraph`, `main_text`, `tables`.

**Files**: 신규 `js/../extract.rs` 또는 `content_extractor.rs`, `oxibrowser` CLI `fetch/extract` 핸들러

**Acceptance**:
- 뉴스 기사 URL에서 `main_text`가 광고·네비 없이 본문만 반환.
- 쇼핑몰 페이지에서 `structured`에 `Product` JSON-LD가 파싱됨.
- 기존 `getMarkdown` 출력과 본문 추출 출력을 비교하는 회귀 테스트.

---

### P1.2 — 관찰자 API: `IntersectionObserver` / `ResizeObserver`  (노력: M · 영향: 중간)

**What**: 두 관찰자를 구현한다. **의미론적**으로 — 실제 레이아웃이 없으므로 픽셀이 아닌
"보임/안 보임" 상태로.

**Why**: 무한 스크롤·지연 로딩(`loading="lazy"` + IntersectionObserver 콜백)·동적 레이아웃
반응형 사이트가 동작하려면 필요하다. MutationObserver 패턴(`__moRegistry` + 레지스트리
디스패치)과 동일한 아키텍처로 확장 가능하다.

**How**:
- `IntersectionObserver(cb, opts)`: 레지스트리 등록. `scroll` 이벤트 발생 시(또는
  `evaluate` 직후) 각 관찰 대상의 가시성을 `getBoundingClientRect`/`_visible`로 판정 → 콜백 디스패치.
  비율 임계치는 의미론적 근사("뷰포트 안 + 표시 중" = 1.0).
- `ResizeObserver(cb)`: MutationObserver의 `attributes`/스타일 변경 알림에 연동.
- `disconnect/takeRecords/unobserve` 표준 메서드.

**Files**: `js/runtime.rs`(관찰자 섹션 — `notify_*_observers` 패턴 재사용)

**Acceptance**:
- 무한 스크롤 데모에서 `scroll` 후 새 콘텐츠가 DOM에 추가됨(콜백 통해 fetch 트리거).
- `takeRecords()` 가 보류 레코드를 반환.

---

### P2.1 — 웹 컴포넌트: `customElements` + Shadow DOM  (노력: L · 영향: 중간)

**What**: `customElements.define/get/whenDefined`, `element.attachShadow({mode:'open'})`,
Shadow 트리의 DOM 스냅샷 통합.

**Why**: 웹 컴포넌트 기반 사이트(GitHub, 일부 Lit/Stencil 앱, 디자인 시스템)는 Shadow DOM 안에
콘텐츠를 숨긴다. Shadow가 없으면 `querySelector`가 빈 결과를 반환하고 추출이 실패한다.

**How**:
- `customElements`: 생성자 레지스트리 + `connectedCallback` 훅(DOM 삽입 시 호출).
- `attachShadow`: `DomSnapshot`에 `shadow_root: Option<Vec<DomNode>>` 추가.
  open 모드만 지원(탐지·추출 목적상 closed는 의미 없음).
- `querySelector`/`getComputedStyle`가 Shadow 트리를 투과하도록 확장(또는 별도 경로).
- `::part()`/`::slotted()` 의미론은 의미론적 레이아웃 철학에 맞춰 최소화.

**Files**: `webapi/dom/`, `js/runtime.rs`(attachShadow 네이티브), `js/dom_snapshot.rs`

**Acceptance**:
- Shadow DOM 안에 본문을 넣은 테스트 페이지에서 `getMarkdown`이 본문을 잡아냄.
- `customElements.define` 후 인스턴스 생성 시 `connectedCallback` 호출.

---

### P2.2 — boa ↔ V8 행위 동일성 (스텔스 Level-1의 지렛대)  (노력: M(지속) · 영향: 높음)

**What**: 탐지 스크립트가 노리는 boa/V8 행위 차이를 좁힌다. `Error.stack` 포맷·
`Function.prototype.toString` 네이티브 표기·누락된 생성자(`Proxy`·`WeakRef`·`FinalizationRegistry`)·
`Intl`·정규식 동작을 V8에 근사.

**Why**: P0.1(전송)을 해도, JS 실행 환경 자체가 "boa다"라면 CreepJS/FP-Collect 계열 정밀
탐지가 남는다. 완전한 V8 모방은 불가능하지만, **상위 20개 탐지 벡터**를 좁히는 것만으로
정밀 탐지의 신뢰도를 크게 떨어뜨린다. Level-1 스텔스의 효과를 방어한다.

**How**:
- 탐지 벡터 인벤토리화: CreepJS 오픈소스 기준으로 boa가 현재 노출하는 차이점 catalog.
- `Function.prototype.toString`을 네이티브 함수에 대해 `function name() { [native code] }`로.
- 누락 글로벌 생성자 스텁(`Proxy`는 boa에 있을 수 있음 — 먼저 확인).
- `Error.stack` V8 포맷(`at f (...:N:N)`) 근사.
- 회귀용 지문 비교 벤치마크 추가.

**Files**: `js/runtime.rs`(globals/프로토타입 패치), 신규 `tests/fingerprint_parity.rs`

**Acceptance**:
- 크롤링 금지 사이트가 아닌 **공개 지문 테스트 페이지**(예: CreepJS 결과 페이지)에서
  boa 고유 신호가 50% 이상 감소.

---

### P3 — 자동화 신뢰성: auto-wait / network-idle  (노력: S · 영향: 중간)

**What**: `wait_for`에 `networkidle`/`domcontentloaded`/`load` 대기 조건 추가; 클릭 후 자동 안정화 대기.

**Why**: 에이전트 자동화의 1위 실패 원인은 "요소가 아직 안 그려졌는데 클릭함"이다.
현재는 선택자 대기만 있다. 네트워크 정온 + 렌더 안정 대기가 이결성(flakiness)을 줄인다.

**How**:
- `Tab::wait_for`에 `WaitCondition::{Visible, NetworkIdle, DomContentLoaded, Load}`.
- `networkidle`: 최근 Nms 동안 진행 중 요청 0 (이미 `CapturedResponse` 트래킹 있음).
- `OXI.getPageInfo`에 `loadState` 노출.

**Files**: `tab.rs`(wait_for 확장), `session.rs`(요청 카운터)

**Acceptance**:
- SPA 로딩 시퀀스에서 `wait_for(NetworkIdle)` 후 클릭이 100회 중 100회 성공.

---

## 3. 의존성 그래프 / 권장 순서

```
P0.1 (HTTP/2)  ──┐
                 ├──▶  P2.2 (boa↔V8 동일성)  ──▶  "정밀 탐지 통과"
P0.1 (TLS 정합) ─┘  (Level-1 스텔스 JS 표면은 본 세션 구현)  │
                                                            ▼
                               P0.3 (managed 챌린지 솔버) ──▶  "Cloudflare managed 200"
                                   (cf_clearance 루프 · P0.1∩P2.2 동시 필요)
P0.2 (SPA 라우팅) ──▶  P1.2 (관찰자) ──▶  "현대 사이트 동작"
                                       ──▶  P2.1 (웹 컴포넌트)
P1.1 (추출 충실도)  ──▶  "에이전트 1차 가치"
P3 (auto-wait)      ──▶  "자동화 신뢰성"
```

- **1차 목표 (v0.16)**: P0.1(HTTP/2) + P0.2(SPA) + P1.1(추출) + P3(auto-wait).
  이 조합이 **"차단 안 되고 + SPA 동작하고 + 본문 잘 뽑고 + 안정적"** = 실사용 임계점.
- **2차 목표 (v0.17)**: P0.1(TLS 정합) + P2.2(boa 동일성). 정밀 탐지 통과.
- **3차 목표 (v0.18)**: P1.2 + P2.1 + P0.3(managed 챌린지 솔버, P2.2 선행). 더 넓은 사이트 호환 + 챌린지 보호 사이트 200.

---

## 4. "실제 웹 준비" 완료 기준 (Definition of Done)

아래 시나리오 5개가 통과하면 "실제 웹 ready"로 선언한다:

1. **수동 핑거프린트 티어** Cloudflare 보호 페이지가 200 (P0.1). 테스트 타깃은 managed 챌린지를 *사용하지 않음*을 사전 확인(챌린지 인터스티셜/`cf-mitigated` 헤더 부재) — 타깃 선택으로 성공 기준이 게임되지 않도록. managed/JS 챌린지 타깃은 **P0.3**(P0.1 ∩ P2.2 선행)으로 추적.
2. **React Router 데모**에서 링크 탐색 후 콘텐츠 갱신 (P0.2).
3. **뉴스 기사**에서 본문만 마크다운 추출 (P1.1).
4. **무한 스크롤 목록**에서 스크롤 후 N개 초과 수집 (P1.2).
5. **자동화 시나리오**가 100회 반복 시 flake < 2% (P3).

각 시나리오는 회귀 테스트(오프라인 fixture 우선 + 실사이트 `--ignored` 통합)로 유지한다.

---

## 5. 포지셔닝 메모

- **"Zero C deps"와 스텔스의 긴장**: P0.1의 TLS 모방(A안, boring)은 C 의존을 가져온다.
  이를 **옵션 피처(`tls-impersonate`, 기본 OFF)**로 격리하면 기본 배포는 순수-Rust를 유지하고,
  "최대 스텔스"가 필요한 사용자만 옵션을 켠다. 두 마리 토끼 포지셔닝.
- **경쟁 우위**: 순수-Rust + CDP 호환 + 단일 바이너리 조합은 희귀하다. 위 5개 시나리오를
  통과하면 "가벼운 Chrome 대체재" 후보가 아니라 **"AI 에이전트용 1순위 헤드리스"** 자리를 단정한다.
