# OxiBrowser 심층 분석 보고서

**날짜:** 2026-05-14  
**분석 범위:** 전체 코드베이스 (~15,000줄 Rust, 4개 크레이트)  
**분석 방법:** 10개 병렬 서브에이전트 + 정적 분석 + 빌드/테스트 실행

---

## 1. 개요

OxiBrowser는 Rust로 작성된 헤드리스 브라우저로, Chrome DevTools Protocol(CDP) 호환 WebSocket 서버와 boa_engine 기반 JavaScript 런타임을 갖추고 있다. 아키텍처는 Browser→Session→Page→Frame 계층 구조를 따르며, CDP 서버, 네트워크 계층, DOM 파싱, CSS 렌더링 모듈로 구성된다.

**현재 구현 상태 요약:**

| 컴포넌트 | 상태 | 비고 |
|---------|------|------|
| Browser/Session/Page/Frame | ✅ | 라이프사이클 관리 완료 |
| JS Runtime (boa_engine) | ✅ | ES2024+, 4945줄 |
| CDP Server | ⚠️ | 프로토콜 구현 불완전 |
| CDP Domain Handlers | ⚠️ | Puppeteer/Playwright 호환성 부족 |
| Network (HTTP client) | ⚠️ | 쿠키/SSRF 문제 |
| Cookie Jar | ⚠️ | RFC 6265 미준수 |
| robots.txt | ❌ |=dead code |
| IpFilter | ❌ |dead code (SSRF 보호 없음) |
| CSS Rendering | ❌ | 텍스트 더미, CSS 없음 |
| Screenshot | ⚠️ | 텍스트 래스터라이저 |
| Test Suite | ⚠️ | 17개 파일 테스트 없음 |

---

## 2. 핵심 발견사항

### 2.1 보안 (🔴 CRITICAL - 3건, 🟠 HIGH - 6건)

#### VULN-01: SSRF 보호 미적용 — IpFilter는 데드 코드
- **심각도:** 🔴 CRITICAL
- **위치:** `network/ip_filter.rs`, `network/client.rs` 전체

`IpFilter::block_private()`이 정의되어 있으나 **어디에도 사용되지 않음**. HTTP 요청 전 IP 체크가 전혀 없으므로, 임의의 URL로 내부 서비스 접근 가능.

```
공격 시나리오:
1. page.navigate("http://169.254.169.254/latest/meta-data/")
2. fetch("http://127.0.0.1:22/")
3. JavaScript로 임의의 내부 IP 스캔
```

**권장사항:** `HttpClient::fetch()`에 DNS resolution + IP 필터링 적용.

#### VULN-02: DNS Rebinding 공격 가능
- **심각도:** 🔴 CRITICAL
- `reqwest`가 내부 DNS resolver를 사용하며 커스텀 핀닝 없음. TTL이 짧은 DNS 레코드로 프라이빗IP ↔ 퍼블릭IP 교대 시 SSRF 우회 가능.

#### VULN-03: IPv6 미처리
- **심각도:** 🟠 HIGH
- `ip_filter.rs`에서 `IpAddr::V6(_) => false` — 모든 IPv6 주소가 필터 우회. `::1`(루프백), `fd00::/7`(ULA), `fe80::/10`(링크로컬) 모두 통과.

#### VULN-04: crypto.getRandomValues 예측 가능한 "난수"
- **심각도:** 🔴 CRITICAL
- **위치:** `js/runtime.rs:~1850-1865`
```rust
let val = (std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .subsec_nanos() as u8)
    .wrapping_add(i as u8);
```
`SystemTime::now().subsec_nanos()`는 결정적. 공격자가 실행 시간을 알면 출력을 예측 가능. 토큰, nonce, 키 material 생성에 사용 시 치명적.

**권장사항:** `rand::rngs::OsRng` 또는 `getrandom::getrandom()` 사용.

#### VULN-05: JS sandbox escape via fetch()
- **심각도:** 🟠 HIGH
- JavaScript의 `fetch()`/`XMLHttpRequest`에 URL 유효성 검증, SSRF 필터링, 프로토콜 제한 없음.

#### VULN-06: ctx.eval()에 비escaped 문자열 주입
- **심각도:** 🟠 HIGH
- **위치:** `js/runtime.rs` lines ~1007, ~1029, ~1039, ~1083
```rust
let reject_code = format!(
    "Promise.reject(new Error('{}'))",
    e  // 에러 메시지에 따옴표/역슬래시 포함 가능
);
```
HTTP 응답 본문이나 에러 메시지에 `</script>`, `');alert(1)//` 포함 시 JS 코드 주입 가능.

**권장사항:** 모든 문자열Interpolate에 `serde_json::to_string()` 사용.

#### VULN-07: 쿠키 보안 — SameSite/Secure 미적용
- **심각도:** 🟠 HIGH
- **위치:** `network/cookie.rs` 전체
  - SameSite attribute 없음 (CSRF 방어 완전 부재)
  - Secure flag 미체크 (HTTPS 쿠키가 HTTP로 전송)
  - Domain matching 완전 잘못됨 (`url.host_str()`만 사용, 쿠키의 Domain attribute 무시)
  - Path matching 미구현
  - 도메인 검증 없어서 `evil.com`이 `Domain=.bank.com` 쿠키 설정 가능

#### VULN-08: WebSocket 인증/Origin 체크 없음
- **심각도:** 🟠 HIGH
- **위치:** `server.rs:158-230`
- `--host 0.0.0.0` 바인딩 시 네트워크 노출, Origin 헤더 미검증, 인증 없음.
- 크로스사이트 WebSocket 하이재킹 가능.

#### VULN-09: TLS 인증서 검증 비활성화 가능
- **심각도:** 🟠 HIGH
- `accept_invalid_certs` 플래그로 MITM 공격 가능. 기본값은 안전하나 경고 없음.

### 2.2 데이터 무결성 / 논리 버그

#### BUG-01: append_child 재부모 지정 시 트리 손상
- **심각도:** 🔴 CRITICAL
- **위치:** `tree.rs:34-37`
```rust
pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
    self.parents.insert(child, parent);  // 기존 부모 덮어씀
    self.children.entry(parent).or_default().push(child);
    // BUG: 기존 부모의 children 목록에서 제거 안 함 — 자식이 두 부모에!
}
```
자식이 두 부모의 children 목록에 동시에 존재하는 이상 상태 발생.

#### BUG-02: traverse_bfs가 실제 DFS
- **심각도:** 🔴 CRITICAL
- **위치:** `tree.rs:69-82`
- `Vec::pop()` 사용 (LIFO = DFS)인데 `traverse_bfs`로 이름 지어짐. 테스트도 DFS 순서 검증.

#### BUG-03: robots.txt lookup이 동작하지 않음
- **심각도:** 🔴 CRITICAL
- **위치:** `robots.rs:67-78`
- `rules` 맵이 도메인으로 키되어 있는데, `is_allowed()`가 **user-agent 문자열로 조회**. 맵 키와 조회 키가 다름 — 룰을 절대 찾지 못함.
- `rules_for_agent()`도 `_agent` 파라미터를 무시하고 모든 에이전트가同一 ruleset 공유.

#### BUG-04: QualName 메모리 leak
- **심각도:** 🔴 CRITICAL
- **위치:** `document.rs:599`
- 모든 `create_element()` 호출마다 `Box::leak(Box::new(name))`. 10K 요소 문서당 ~1-2MB leak, 프로세스 종료 시까지 지속.

#### BUG-05: remove_from_parent/reparent_children 미구현
- **심각도:** 🔴 CRITICAL
- **위치:** `document.rs:672, 676`
- html5ever가 호출하는 TreeSink 콜백이 빈 no-op. 잘못된 HTML 파싱 시 트리 구조 손상.

#### BUG-06: max_sessions 체크 레이스 컨디션
- **심각도:** 🔴 CRITICAL
- **위치:** `browser.rs:93-99`
```rust
if self.sessions.read().len() >= self.config.max_sessions {  // 체크
    return Err(...);
}
let session = Session::new(...).await?;  // 세션 생성
self.sessions.write().push(session.clone());  // 푸시
// → 두 스레드가 동시에 통과 가능 → max_sessions + 1 세션
```

#### BUG-07: getAttribute가 생성된 요소에서 stale data 반환
- **심각도:** 🟠 HIGH
- **위치:** `js/runtime.rs:~2430-2440`
- `createElement` 시 `attrs_map.clone()`으로 빈 HashMap 캡처. `setAttribute` 호출 시 DomMutation만 기록하고 `attrs_map` 미업데이트. 이후 `getAttribute`는 항상 `null` 반환.

#### BUG-08: URL.searchParams getter가 undefined 반환
- **심각도:** 🟠 HIGH
- **위치:** `js/runtime.rs:~1718-1728`
```rust
let _query = search.trim_start_matches('?').to_string();
Ok(JsValue::undefined())  // <-- URLSearchParams 대신 undefined 반환
```

#### BUG-09: innerHTML = textContent
- **심각도:** 🟠 HIGH
- **위치:** `js/runtime.rs:~2970`
- 자식을 가진 요소의 `innerHTML`이 텍스트만 반환 (HTML 직렬화 없음).

#### BUG-10: removeEventListener가 이벤트 타입의 모든 리스너 제거
- **심각도:** 🟠 HIGH
- **위치:** `js/runtime.rs:~2120-2135`
```rust
let _ = l_obj.set(JsString::from(event_type.as_str()), JsValue::Null, true, ctx);
// 실제 removeEventListener: 특정 콜백만 제거
// 현재 구현: 이벤트 타입의 모든 리스너 제거
```

### 2.3 API 미완성 (Puppeteer/Playwright 호환성)

#### MISSING-01: Runtime.callFunctionOn 미구현
- **심각도:** 🔴 CRITICAL
- **위치:** `domains/runtime.rs:89-101`
- 항상 `undefined` 반환. Puppeteer의 `element.click()`, `element.type()`, `element.evaluate()` 전부가 이 메서드 사용 — **Puppeteer 요소 조작 완전 불능**.

#### MISSING-02: Target.setDiscoverTargets 이벤트 미발생
- **심각도:** 🔴 CRITICAL
- **위치:** `domains/target.rs:16-19`
- `Target.targetCreated` 이벤트 미발생. Puppeteer 연결 설정 헤더딩.

#### MISSING-03: DOM.describeNode stub 반환
- **심각도:** 🟠 HIGH
- **위치:** `domains/dom.rs:103-117`
- 항상 `nodeType: 1, nodeName: "BODY"` 반환 — 실제 노드 데이터 무시.

#### MISSING-04: DOM.resolveNode disconnected objectId
- **심각도:** 🟠 HIGH
- **위치:** `domains/dom.rs:119-130`
- UUID 기반 objectId 생성, JS 런타임과 연결 없음. `callFunctionOn`과 연결 불可能.

#### MISSING-05: Emulation.setDeviceMetricsOverride 미구현
- **심각도:** 🟠 HIGH
- Puppeteer이 페이지 생성 시 항상 호출. 전체 Emulation 도메인 없음.

#### MISSING-06: Page.setViewport 미구현
- **심각도:** 🟠 HIGH
- Puppeteer의 `page.setViewport()` 매핑 안 됨.

#### MISSING-07: Page.addScriptToEvaluateOnNewDocument 미구현
- **심각도:** 🟠 HIGH
- Puppeteer이 주입 스크립트 설치용으로 사용.

#### MISSING-08: Fetch.enable 기본 패턴 문제
- **심각도:** 🟠 HIGH
- **위치:** `domains/fetch.rs:85-86`
- 패턴 없을 때 `*[FetchPattern::default()]` 설정 — 모든 요청 가로챔.

### 2.4 CSS 선택자 미완성

| 선택자 | 지원 여부 |
|--------|----------|
| tag, .class, #id | ✅ |
| tag.class, tag#id | ✅ |
| Combinators (>, +, ~, 공백) | ❌ |
| Attribute selectors | ❌ |
| Pseudo-classes (:first-child, :nth-child, :not, etc.) | ❌ |
| Multiple selectors (a, b) | ❌ |
| Universal selector (*) | ❌ |

JS runtime의 `dom_snapshot.rs`도 동일한 제한.

### 2.5 CSS 렌더링 — 실제 CSS 없음

`css/` 모듈은 DOM 텍스트 직렬화 + 비트맵 텍스트 래스터라이저일 뿐:
- CSS 속성( display, margin, padding, color 등) 미적용
- `render_to_markdown`이 실제 마크다운 형식 출력 안 함 (무시되는 `_skip` 변수)
- 스크립트/스타일 콘텐츠가 마크다운에 누출
- 이미지, 색상, 레이아웃 미지원
- `captureScreenshot`의 `format`, `quality`, `clip` 파라미터 무시
- ASCII-only 폰트 (95글자), Unicode 렌더링 불가
- 뷰포트 1280×10000px 이미지에서 최대 819MB 메모리 할당 → OOM 위험

### 2.6 네트워크 계층

| 문제 | 설명 |
|------|------|
| 단일 Set-Cookie 헤더만 캡처 | HTTP 응답의 여러 Set-Cookie 헤더 중 첫 번째만 저장 |
| reqwest 에러 → String 손실 | 구조화된 에러 정보 (timeout vs DNS failure) 완전 소실 |
| Intercept가 POST → GET 변환 | 메서드 오버라이드 없을 시 모든 요청이 GET으로 변경 |
| intercept requestId 타임아웃 없음 | CDP 클라이언트 미응답 시 무한 대기 |
| 네트워크 이벤트 순서 잘못됨 | Page 이벤트 후 Network 이벤트 발생 (정순서: Network → Page) |

---

## 3. 테스트 스위트 분석

### 3.1 커버리지

| 파일 | 테스트 수 | 평가 |
|------|----------|------|
| `js/runtime.rs` (4945줄) | 83 | ⭐ Excellent |
| `encoding.rs` | 30 | ⭐ Good |
| `network/cookie.rs` | 13 | ✅ Good |
| `webapi/dom/document.rs` | 14 | ✅ Good |
| `js/dom_snapshot.rs` | 14 | ✅ Good |
| **browser.rs** (223줄) | **0** | ❌ |
| **session.rs** (772줄) | **0** | ❌ |
| **page.rs** (143줄) | **0** | ❌ |
| **network/client.rs** (230줄) | **0** | ❌ |
| **robots.rs** (121줄) | **0** | ❌ |
| E2E tests | 23 | ✅ |
| Smoke tests | 3 | ✅ |
| Integration tests (#[ignore]) | 9 | ✅ |

**테스트 없는 파일:** 17개 (코드베이스 가장 중요한 세션/브라우저/네트워크 클라이언트 포함)

### 3.2 플레이키 테스트 리스크

| 위험 | 영향 | 완화 |
|------|------|------|
| `sleep(50ms)` 서버 시작 대기 | 모든 E2E 테스트 (23개) | 리트라이 루프 사용 필요 |
| 고정 event collection window | 500ms-1000ms 타이밍 의존 | CI에서 실패 가능 |
| 프로세스 스폰 컴파일 대기 | smoke 테스트 3개 | 사전 빌드 사용 필요 |

### 3.3 테스트 품질 이슈

- `test_fetch_fulfill_request`: Fetch.requestPaused 수신 확인 안 함
- `test_full_workflow`: `1+1` 결과가 `number or string`만 검증 — 너무 관대
- `test_korean_encoding_naver`: ASCII fallback으로 인코딩 테스트 의미 없음
- 26개의 `.unwrap()` E2E 테스트에서 인프라 실패 시 panic

---

## 4. 의존성 관리

### 4.1 빌드/린트 상태

| 항목 | 결과 |
|------|------|
| `cargo build --workspace` | ✅ Zero warnings |
| `cargo clippy --workspace` | ✅ Zero warnings |
| `cargo test --workspace` | ✅ 182 passed, 0 failed |

### 4.2 의존성 중복 (56개 인스턴스, 28개 고유)

| 카테고리 | 버전 수 | 원인 |
|---------|--------|------|
| hashbrown | **4** (0.14, 0.15, 0.16, 0.17) | boa, regress, indexmap, dashmap |
| ICU4X | **2** (v1 vs v2) | boa가 v1, 새 deps가 v2 |
| rand | **2** (0.8 vs 0.9) | boa vs tungstenite |
| getrandom | **3** (0.2, 0.3, 0.4) | rand chain |

**행동 가능 여부:** 거의 전부 상위 transitive deps 문제, OxiBrowser 수준에서 해결 불가 (boa_engine 업데이트 대기).

### 4.3 workspace 관리 미흡

- `clap`, `pollster`, `criterion`이 workspace dep 아닌 개별 크레이트에 직접 핀ning
- 내부 크레이트 의존성에 `path` 누락 (`oxibrowser-core = "0.6.0"` 대신 `{ version = "0.6.0", path = "../oxibrowser-core" }` 필요)

---

## 5. 발견사항 종합 통계

| 카테고리 | 🔴 CRITICAL | 🟠 HIGH | 🟡 MEDIUM | 🟢 LOW | 합계 |
|---------|------------|---------|-----------|--------|------|
| 보안 | 3 | 6 | 10 | 3 | **22** |
| 데이터 무결성/버그 | 6 | 5 | 12 | 4 | **27** |
| API 미완성 | 2 | 5 | 6 | 3 | **16** |
| CSS 선택자 | 0 | 1 | 2 | 0 | **3** |
| CSS 렌더링 | 2 | 5 | 10 | 5 | **22** |
| 네트워크 | 0 | 1 | 5 | 2 | **8** |
| 테스트 | 0 | 2 | 3 | 4 | **9** |
| 의존성 | 0 | 0 | 2 | 1 | **3** |
| **합계** | **13** | **25** | **50** | **22** | **110** |

---

## 6. 우선순위별 권장사항

### P0 — 즉시 수정 (보안/데이터 무결성)

1. **IpFilter 활성화** — `HttpClient::fetch()`에 DNS resolution + IP 필터링 적용. IPv6 지원 추가.
2. **crypto.getRandomValues 수정** — `rand::rngs::OsRng` 사용.
3. **ctx.eval 문자열 escaping 수정** — `serde_json::to_string()` 모든 주입 지점 적용.
4. **tree.rs append_child 버그 수정** — 기존 부모 children 목록에서 제거.
5. **traverse_bfs → traverse_dfs로 이름 수정** — 구현은 이미 DFS.
6. **robots.rs lookup 로직 수정** — 도메인 기반 조회, 에이전트별 규칙 격리.
7. **QualName leak 수정** — `Box::leak` 제거, Document 라이프사이클 관리.
8. **session.rs race condition 수정** — write lock 안에서 max_sessions 체크.

### P1 — 단기 개선 (Puppeteer/Playwright 호환성)

9. **Runtime.callFunctionOn 구현** — Puppeteer 요소 조작의 핵심.
10. **Target.setDiscoverTargets 이벤트 발생** — `Target.targetCreated` emitted.
11. **DOM.describeNode 실제 노드 데이터 반환** — stub 제거.
12. **DOM.resolveNode JS objectId 연결** — `callFunctionOn`과 연계.
13. **Page.setViewport 구현** — Emulation 도메인 추가.
14. **Network events 순서 수정** — Network → Page 순서 준수.
15. **Fetch.enable 기본 패턴 수정** — 빈 패턴 시 가로채기 활성화 안 함.

### P2 — 중기 개선 (품질/완전성)

16. **session.rs/browser.rs/page.rs 단위 테스트 추가** — 가장 큰 테스트 없는 파일.
17. **Cookie RFC 6265 구현** — SameSite, Secure, Domain, Path enforcement.
18. **CSS 선택자 완전 구현** — combinators, attribute selectors, pseudo-classes.
19. **CSS 렌더링 모듈 재설계** — 실제 CSS 파싱/적용 (장기 로드맵).
20. **Screenshot OOM 방지** — 최대 높이 제한, 스트리밍考虑.
21. **E2E 테스트 커버리지 확대** — OXI domain, captureScreenshot, navigation errors.

### P3 — 장기 (아키텍처)

22. **robots.txt 실제 통합** — HttpClient::fetch 전에 robots.txt 체크.
23. **CORS/SOP 구현** — 자동화 시나리오의 보안 경계.
24. **rate limiting** — HTTP 및 CDP 요청 제한.
25. **benchmark infra** — JS runtime, CDP server, network benchmarking 추가.

---

## 7. 아키텍처 강점

| 항목 | 평가 |
|------|------|
| Browser→Session→Page→Frame 계층 | ✅ 명확한 책임 분리 |
| JsRuntime thread/channel 모델 | ✅ boa Context (!Send) 문제 올바르게 해결 |
| CDP EventSender/EventReceiver 분기 | ✅ 도메인별 event gating |
| CDP Server (hyper + tungstenite) | ✅ 웹소켓 업그레이드 올바르게 구현 |
| Document (html5ever TreeSink) | ✅ 파싱 구조 우수 |
| Error type 설계 (thiserror) | ✅ 일관된 에러 처리 |
| 테스트 구조 (E2E via tokio-tungstenite) | ✅ 실제 프로토콜 테스트 |

---

## 8. 결론

OxiBrowser는 견고한 아키텍처 기반과 양호한 테스트基础设施를 갖추고 있으나, **보안**, **데이터 무결성**, **Puppeteer/Playwright 호환성**에서严重한 결함이 있다. 가장 긴급한 문제는:

1. **SSRF 보호 완전 부재** (IpFilter가 데드 코드)
2. **crypto.getRandomValues 예측 가능** (보안용 난수 불가능)
3. **Puppeteer 요소 조작 불가** (callFunctionOn 미구현)
4. **쿠키 보안 미구현** (SameSite/Secure/Domain 전무)
5. **tree 버그** (append_child, traverse_bfs 이름/구현 불일치)

이 수정 없이 OxiBrowser를 외부 네트워크에 노출하거나 보안 민감한 자동화 작업에 사용하면严重한 보안 위험이 있다. 하지만 아키텍처가 건전하므로 이러한 문제 수정이 가능할 것으로 보인다.

---

*본 보고서는 10개 병렬 서브에이전트에 의해 생성됨. 각 서브에이전트는 3-10개 파일을 상세 분석하고 100개 이상의 발견사항을 보고함.*