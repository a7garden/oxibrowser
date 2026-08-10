# OxiBrowser 남은 작업 전부 완료 — 설계 (spec)

> **작성:** 2026-08-10 · **분기:** `main`
> **상위 문서:** `docs/superpowers/specs/2026-08-10-remaining-work.md`, `2026-08-09-post-phase5-remaining-roadmap.md`
> **상위 로드맵:** `docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md`
> **목적:** Phase 1–9가 닫힌 시점에서 **남은 작업 전부**(검증 baseline + 폰트 + iframe long-tail)를
> 완료하기 위한 설계. 이 문서는 brainstorming 산출물이며, `writing-plans` 스킬이 이를 소비해
> 단계별 구현 계획(plan)을 생성한다.

---

## 0. 하드 제약 (상위 로드맵 §5 계승)

- **pure Rust 유지** — Chromium/V8 도입 금지. `boa_engine` / `html5ever` / Blitz 스택(Stylo+Taffy+Parley+vello) / `wreq`+`btls` 유지.
- **비목표:** 픽셀 퍼펙트 Chrome 렌더링 패리티, 안티봇 챌린지 풀이, DevTools 프론트엔드, V8/네이티브 JS엔진.
- **의존성 한계(재개 금지):** `boa` 0.20 위치 정보 부재(실제 소스 레벨 스택 프레임 불가), canvas/WebGL 실제 래스터화(의존성/범위 한계).

---

## 1. 현황 (한 줄)

**Phase 1–9 + Phase 8 iframe 분리 컨텍스트 전부 완료 (2026-08-10, v0.18.0).** 핵심 피드백 루프(parse →
`<script>` 실행 → 이벤트 루프 → 비동기 fetch → 라이브 RenderDocument → Blitz 렌더 → screenshot/CDP)는
닫혀 있고, CDP 프로브는 11/11 PASS. **남은 것: 검증 baseline 부재, 폰트, iframe long-tail.**

### 1.1 두 가지 주요 발견 (이 설계를 재형성함)

**발견 A — 폰트 "Blitz fork 필요" 전제는 거짓.**
`2026-08-10-remaining-work.md` §2.2 / `2026-08-09-post-phase5-remaining-roadmap.md` §3.2 가 `util.rs`의
`FONT_DB`(`pub(crate)` + `svg` 게이트)를 폰트 블로커로 지목했으나, 이는 **`usvg` SVG 렌더링 경로**이지
텍스트 레이아웃이 아니다. 실제 텍스트 폰트 경로는 전부 **공개 API**다 (crates.io `blitz-dom-0.3.0-beta.1`
소스 검증):

| 위치 | 공개 API |
|---|---|
| `config.rs:49` | `pub font_ctx: Option<FontContext>` (`DocumentConfig` 필드) |
| `lib.rs:91-122` | `pub fn build_single_font_ctx(&[u8]) -> FontContext` (WOFF/WOFF2 디코드) |
| `document.rs:356-379` | `BaseDocument::new`가 `config.font_ctx` 소비; `font_ctx.collection.register_fonts(...)` 등록 |
| `font_metrics.rs:16` → `document.rs:338,391` | `font_ctx` → `BlitzFontMetricsProvider` → Stylo `FontMetricsProvider` → 레이아웃 도달 |
| `util.rs:42-51` | `FONT_DB`는 `#[cfg(feature="svg")]` + `usvg::fontdb` (SVG 경로, 텍스트 아님) |

**Spike 검증(2026-08-10, 실행 완료):** 같은 HTML을 두 폰트로 렌더 — `Arial Black` vs `Andale Mono` →
세 픽셀 버퍼 해시 전부 상이(`default=5fd0ed67… / arial_black=a8b7995f… / andale_mono=9a9e6140…`),
`inspect_image`로 두 폰트 모두 "Hello World OxiBrowser"가 정확한 글리프(헤비 산세리프 / 등폭 모노스페이스,
tofu 없음)로 렌더됨 확인. **→ `@font-face`는 fork 없이 구현 가능.**

**발견 B — 외부 `<link rel=stylesheet>`는 현재 fetch/적용되지 않음.**
`ResourceKind::Stylesheet`는 추출되고(`dom_snapshot.rs:1158`) `ResourceType::Stylesheet`로 매핑되나
(`session.rs:1604`), 이는 오직 `resources()` 메타데이터 보고용(Network 도메인). CSS를 fetch해 문서에
주입·적용하는 코드가 없다. 반면 외부 `<script src>`는 fetch·실행됨(`session.rs:1248-1266`). **실제 사이트
폰트는 거의 항상 외부 CSS에 선언**되므로, 외부 stylesheet 적용은 `@font-face`의 **선행 필수 작업**이다.

---

## 2. 범위 (사용자 확정)

- **전부 진행** — 검증 baseline + 폰트 + iframe long-tail(Phase 8 §6 비목표 4종 전부 포함).
- **폰트 전략:** 공개 API 구현(fork/vendoring 없음). spike로 end-to-end 증명됨.
- **인수 테스트 대상:** 로컬 mock React SPA(결정론적·오프라인·회귀 게이트 가능).
- **시퀀싱:** A — 게이트 우선. **W1 검증 → W2 폰트 → W3 iframe long-tail → 릴리스 v0.19.0.** 각 workstream
  후 인수 게이트 재실행.
- **분해:** W1·W2·W3는 독립 subsystem이므로, `writing-plans`는 이 산출물에서 **workstream별로 별도
  구현 계획(plan)**을 생성한다(W1 검증 → W2 폰트 → W3 iframe long-tail). 각 plan은 단독으로 빌드·테스트
  가능한 소프트웨어를 산출한다.

---

## 3. W1 — 검증 (인수 게이트)

> **목표:** 상위 로드맵 §6 궁극 인수 테스트(React SPA: 이동 → 로그인 폼 → submit → 대시보드 대기 →
> 스크린샷)의 **부재 baseline 증거**를 최초로 확보하고, 이후 workstream의 회귀 게이트로 사용.

### 3.1 W1a — 로컬 mock React SPA 인수 하네스

**구성 요소:**

1. **Mock React SPA** (Vite + React Router). 라우트:
   - `/` (landing) — "Login" 버튼 → `/login`
   - `/login` — username/password 입력 폼 + submit 버튼. submit 시 `fetch('/api/session')` 후
     클라이언트 라우팅 `/dashboard`
   - `/dashboard` — `fetch('/api/data')` 결과를 렌더하는 대시보드 셀렉터(`#dashboard`)
   - 빌드하여 정적 서빙(`bun run build` → `vite preview` 또는 정적 파일 서버). 데이터 API는 하네스가
     제공하는 mock 엔드포인트.
2. **Playwright 드라이버** (`acceptance/run.ts`, Bun 실행). 흐름:
   - `oxibrowser serve`(빌드된 바이너리, `cargo build --features browser --bin oxibrowser`)를 포트에 기동
   - 서버 준비 대기(**run.sh gotcha 수정**: 200×0.2s 루프 + curl 준비 확인 후 진행 — 기존 10s 루프가
     콜드 스타트보다 짧아 WS 연결 전 exit 0 위험)
   - Playwright CDP 연결 → `page.goto(spa_url)` → `waitForSelector('#login-form')` → `fill` → `click`
     → `waitForSelector('#dashboard')` → `screenshot()`
   - 어설션 결과 + `acceptance/baseline.png` + `acceptance/result.json` 출력
3. **run.sh / 진입점** — 빌드 + 서빙 + 드라이버 실행을 한 번에.

**위치:** `acceptance/mock-spa/`(SPA 소스 + 빌드 산출물 gitignored), `acceptance/run.ts`,
`acceptance/run.sh`.

**데이터 플로우:**
```
bun run acceptance/run.sh
  → cargo build --features browser --bin oxibrowser
  → oxibrowser serve :PORT &  (CDP 엔드포인트)
  → vite preview :SPA_PORT &  (mock SPA + /api mock)
  → bun run acceptance/run.ts
       → Playwright CDP → drive flow → assert #dashboard → screenshot
  → result.json {pass, steps, durations}
```

**에러 처리:** 서버 기동 타임아웃 / 요소 미발견 타임아웃 / 스크립트 예외 → 명확한 실패 메시지 + 0이 아닌 종료 코드. 각 단계의 duration을 `result.json`에 기록해 회귀 시 원인 국소화.

**첫 실행의 의미:** 현재 증명 부재([INFERENCE] "대부분 통과할 것으로 예상") 상태에서, **실제로 어디까지
통과하는지 baseline을 기록**. 통과 단계/실패 단계/스크린샷이 명시적 증거가 된다. W2·W3 후 재실행하여
회귀/개선을 비교한다.

### 3.2 W1b — JS-fetch 인터셉션 CDP round-trip e2e

구현·배선·유닛 테스트는 완료(`e031352`)되었으나 라이브 CDP round-trip이 미검증. 독립 Playwright 테스트:
- 페이지: `<script>fetch('/api').then(r=>r.text()).then(t=>window.__res=t)</script>`
- `Fetch.enable({ patterns: [{ urlPattern: '/api' }] })`
- `Fetch.requestPaused` 이벤트 수신 확인 → `Fetch.fulfillRequest({ requestId, body: mockBody })`
- 페이지 eval `window.__res` → mockBody와 일치 확인

**위치:** `acceptance/fetch-intercept.ts`.

---

## 4. W2 — 폰트 (`@font-face`, 공개 API, fork 없음)

> **목표:** `@font-face`로 선언된 웹폰트를 fetch하여 Parley FontContext에 등록하고, 정확한 폰트로
> 레이아웃·랜더링. spike로 증명된 공개 API 경로. fork/vendoring 불필요.

### 4.1 W2-pre — 외부 stylesheet 적용 (선행 필수)

발견 B에 의해, 외부 CSS에 선언된 `@font-face`가 동작하려면 외부 stylesheet를 fetch해 문서에 적용해야 한다.
이는 `@font-face`뿐 아니라 **일반 렌더링 충실도**에도 영향(현재 외부 CSS가 전혀 적용 안 됨).

**접근:** 외부 `<script src>` fetch 패턴(`session.rs:1248-1266`)을 폰트·CSS로 일반화.
- navigate에서 `extract_resource_urls()`의 `Stylesheet` 종류를 fetch
- CSS 텍스트를 얻은 뒤, 이를 문서에 주입. 두 가지 적용 경로 후보(plan에서 확정):
  - (a) fetch한 CSS 텍스트를 HTML의 `<style>`로 인라인화 후 재파싱, 또는
  - (b) Blitz의 stylesheet API에 직접 추가(`BaseDocument`가 노출하는 stylesheet 추가 경유)
- fetch 실패 시 warn + 스킵(네비게이션 중단 금지, 기존 패턴)

> **스코프 게이트:** 이 sub-task는 "외부 stylesheet 적용"이라는 별도 충실도 갭을 다룬다. 본 설계에 포함시키는
> 근거는 유저의 "전부" 의도 + 외부 CSS @font-face의 실용적 필수성. plan 단계에서 (a)/(b) 적용 경로를
> 확정한다.

### 4.2 W2-core — `@font-face` 추출 → fetch → register → 주입

**구성 요소:**

1. **`@font-face` 추출.** CSS 텍스트(inline `<style>` + 4.1에서 fetch한 외부 CSS)에서
   `@font-face { font-family: <name>; src: url(<url>) [format(...)]; }`를 파싱 →
   `(family_name, url, [weight, style])` 목록. CSS 텍스트이므로 경량 파서/스캐너로 충분(Stylo는 규칙을
   소비하나, 우리는 fetch를 위해 URL을 사전 추출해야).
2. **폰트 fetch.** 각 URL을 base_url로 resolve → 기존 네트워크 스택(`<script src>`와 동일 bridge)로 폰트
   바이트 fetch. WOFF/WOFF2는 `blitz_dom::decode_font_bytes`(공개, `lib.rs:89` re-export)로 디코드.
3. **FontContext 조립.** `parley::FontContext`의 `collection.register_fonts(Blob::new(bytes), Some(attrs))`로
   family별 등록. `build_single_font_ctx`(universal fallback)이 아니라 **정확한 family 매핑**으로 —
   `@font-face`의 `font-family` 이름이 레이아웃 시 해당 폰트로 해결되도록.
4. **문서 생성에 주입.** 신규 메서드
   `RenderDocument::from_html_with_font_ctx(html, base_url, viewport, font_ctx: parley::FontContext)`.
   기존 `from_html`는 `font_ctx=None`으로 위임. 훅 지점: `session.rs:781` `Page::from_html` 호출 전에
   FontContext 빌드하여 전달(`Page::from_html`도 `Option<FontContext>` 매개변수 추가). render crate에
   `parley` 직접 의존성 추가(workspace 경유 전이적 존재).
5. **재레이아웃.** 폰트를 생성 **전**에 fetch·등록하므로 첫 `doc.resolve(0.0)`가 올바른 폰트로 배치.
   v1은 동기(블로킹 fetch 후 레이아웃). 비동기 FOUT/FOIT/font-swap은 v2 비목표.

**데이터 플로우:**
```
navigate → html(767)
  → [W2-pre] fetch 외부 Stylesheet CSS → [W2-core] inline + 외부 CSS에서 @font-face URL 추출
  → 폰트 fetch(네트워크) → decode_font_bytes → FontContext(register_fonts per family)
  → Page::from_html(.., Some(font_ctx)) → RenderDocument::from_html_with_font_ctx
  → resolve(0.0) 올바른 폰트 배치 → screenshot/CDP
```

**에러 처리:** 폰트 fetch 실패 → 해당 폰트 스킵(warn 로그), 시스템 폰트 fallback. 폰트 하나가 네비게이션을
중단시키지 않음(iframe/script 패턴과 동일). 파싱 불가능한 `@font-face` 규칙은 스킵.

**테스트:**
- 단위: `@font-face` 파서가 샘플 CSS에서 정확한 `(family, url, weight/style)` 추출.
- 통합: 알려진 `@font-face`(repo에 번들한 결정적 테스트 폰트) 페이지 렌더 → 글리프 해시 비교(spike와 동일
  원리: 주입 폰트 vs 시스템 폰트가 상이한 픽셀 생성). 결정성을 위해 테스트 폰트를 repo에 번들
  (`crates/oxibrowser-render/tests/fixtures/`).
- 인수 게이트: W1 SPA에 웹폰트를 사용하도록 mock을 수정하지 않음(회귀 비교 안정성). 대신 별도 통합 테스트로
  검증.

---

## 5. W3 — iframe long-tail (Phase 8 §6 비목표)

> **목표:** Phase 8 v1이 비목표로 둔 4종을 완료하여 iframe 패러다이의 마지막 간극을 닫는다.
> 상세 아키텍처는 `2026-08-10-phase8-iframe-contexts.md`(이미 구현된 per-frame 컨텍스트 기반)에 의존.

**내부 순서: W3a → W3b → W3d → W3c**(가장 복잡한 `window.parent/top`을 마지막).

### 5.1 W3a — `srcdoc` / `about:blank` iframe (최소 노력·최대 가치)

`populate_iframes`가 현재 non-http(s)를 스킵(`session.rs:872-874`).
- `<iframe srcdoc="<html>...">` → srcdoc HTML 문자열로 자식 `Frame` 빌드(fetch 없음).
- `<iframe src="about:blank">` → 최소 빈 HTML로 자식 문서.
- 둘 다 기존 `inject_child_frames`(`session.rs:1305+`) 경로로 자식 컨텍스트 생성(로컬 HTML 사용).
- `extract_resource_urls`(`dom_snapshot.rs:1134`)가 `srcdoc` 속성을 surface하도록 확장 + `populate_iframes`가
  kind에 따라 분기(http: fetch / srcdoc: 로컬 / about:blank: 빈).

### 5.2 W3b — 중첩 iframe (재귀)

`populate_iframes`가 루트 자식만 처리 → 각 자식 `Frame`의 문서로 재귀하여 중첩 `<iframe>`을 발견.
- per-context 아키텍처(`context_id` 임의 증가)가 임의 depth를 구조적으로 지원.
- `inject_child_frames`를 재귀 확장(자식의 자식도 `SetFrameDocument`로 컨텍스트 생성).
- **사이클 방지:** depth cap(예: 5) + 자기 참조 iframe 탐지. 초과 시 warn + 스킵.

### 5.3 W3d — 동적 iframe 추가/제거 추적

JS가 iframe을 동적으로 생성(`document.createElement('iframe')` + `appendChild`, 또는 기존 iframe의
`.src` 변경)할 때, 기존 DOM mutation 훅(`DomMutation` / `appendChild` / `setAttribute` 경로)으로 감지 →
해당 자식 컨텍스트를 생성하거나 제거.
- 라이브 `RenderDocument`의 iframe 요소 mutation 관찰.
- iframe 제거 시 자식 컨텍스트 drop(메모리 정리).
- 동적 `srcdoc`/`src` 변경도 W3a/W3b와 동일 분기로 처리.

### 5.4 W3c — `window.parent` / `window.top` (가장 복잡)

크로스-프레임 window 프록시. 각 자식 `boa::Context`가 격리되어 있어, `window.parent`는 프로퍼티 접근 시
**부모 컨텍스트로 `JsCommand::Eval`을 전달·대기**하는 native object를 반환해야 한다.
- `window.parent` → 소유 parent `context_id`를 아는 프록시 객체.
- 프로퍼티 get/set/메서드 호출 → JS 스레드에 `Eval { context_id: parent_id, expression }` 전송 → 결과를
  boa 값으로 역직렬화. 예: `window.parent.document.title` → 부모에서 `document.title` eval.
- `window.top` → `context_id=1`까지 parent 체인을 워크.
- `window.postMessage(msg, targetOrigin)` → 타겟 컨텍스트에서 `message` 이벤트 발화.
- **same-origin 강제는 v1 비목표 유지**(Phase 8 §6). 모든 프레임이 동일 세션 쿠키/스토리지 공유.
- **복잡성:** 채널 round-trip(동기 대기)이 있으므로 bounded but real. 프로퍼티 접근 직렬화 전략을
  plan에서 확정(전체 객체 복사 vs 프리미티브만).

### 5.5 W3 공통 에러 처리

깨진 iframe / fetch 실패 / 컨텍스트 생성 실패 → warn + 스킵. 단일 iframe이 네비게이션이나 다른 프레임을
중단시키지 않음(기존 패턴 준수).

---

## 6. 공통 (cross-cutting)

### 6.1 릴리스
- 워크스페이스 버전 0.18.0 → 0.19.0(4 crate).
- `CHANGELOG.md`에 `[0.19.0]` 섹션 추가(인수 게이트, 외부 CSS 적용, @font-face, iframe long-tail).
- 태그.

### 6.2 문서 정정
- `2026-08-10-remaining-work.md` §2.2: 폰트 블로커 기재 정정(`FONT_DB`/svg 혼동 → 공개 API 구현 가능, spike 증거).
- `2026-08-09-post-phase5-remaining-roadmap.md` §3.2: 동일 정정.
- 발견 B(외부 stylesheet 미적용)를 새로운 알려진 갭으로 기록.

### 6.3 검증 게이트 (매 workstream 커밋)
```bash
cargo build --features browser --bin oxibrowser   # browser 피처 필수(stale 바이너리 주의)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bun run acceptance/run.sh                          # 인수 게이트(W1 완료 후부터)
```

---

## 7. 리스크 & 완화

| 리스크 | 영향 | 완화 |
|---|---|---|
| W1 서버 기동 타임아웃(문서화된 gotcha) | 중간 | 200×0.2s 대기 루프 + curl 준비 확인; result.json에 duration 기록 |
| W1 Playwright 의존성 추가 | 낮음 | acceptance는 cargo test와 분리(Bun 실행); CI 별도 잡 |
| W2-pre 외부 CSS 적용 경로 (a)/(b) 미확정 | 중간 | plan 단계에서 스파이크로 (a) 인라인화 vs (b) stylesheet API 결정 |
| W2 `register_fonts` family 매핑 정확도 | 중간 | 정확한 `FontAttrs`(family/weight/style) 전달; 통합 테스트로 검증 |
| W3c cross-context 동기 round-trip 교착 | 중간 | 부모가 자식을 기다리는 중 자식이 부모를 호출하는 재진입 방지; 타임아웃 |
| W3b 중첩 iframe 무한 루프 | 중간 | depth cap 5 + 자기 참조 탐지 |
| 인수 게이트가 W2/W3 전에 실패를 드러냄 | 낮음 | W1 baseline이 "현재 어디까지 통과"를 기록 → 예상치 못한 실패는 발견이지 회귀가 아님 |

---

## 8. 비목표 (본 설계)

- 폰트 비동기 로딩(FOUT/FOIT/font-swap) — v1은 동기 블로킹 fetch.
- `window.parent`/`top`의 same-origin 강제.
- WASM/ESM 모듈, 서비스 워커(별도 갭).
- 픽셀 퍼펙트 Chrome 패리티(상위 로드맵 §5 계승).

---

## 9. 사용자 결정 이력 (brainstorming)

1. 범위 = **전부**(검증 + iframe long-tail + 폰트).
2. 폰트 전략 = **공개 API 구현**(최초 "hybrid fork"는 거짓 전제로 인해 재결정 → spike 증거로 fork 불필요 확정).
3. 인수 테스트 대상 = **로컬 mock React SPA**.
4. 시퀀싱 = **A 게이트 우선**(W1 → W2 → W3 → 릴리스).

끝.
