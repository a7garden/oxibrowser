# OxiBrowser 스텔스 설계서 (wreq 기반)

> **갱신 2026-06-25**: 본 문서는 최초 "pure-Rust(rustls 포크)" 접근으로 기획되었으나, 유지보수자가 pure-Rust 제약을 완화("외부 의존성 허용, 헤드리스 완성도 최우선")하여 **wreq**(BoringSSL 기반 reqwest 하드 포크)로 전환. 전송 스텔스(TLS/H2/헤더)는 wreq가 담당 → 수제 rustls 포크·ECH 벽·커스텀 h1 인코더 계획은 **전부 기각**. 남은 수제 영역은 JS 표면(boa↔V8 동일성)과 managed 챌린지 솔버뿐.

---

## 1. 전략 전환: pure-Rust → wreq

**근거**: pure-Rust로 Chrome JA3 정합을 하려면 rustls 포크(확장 방출 재작성) + ECH(HPKE) + 커스텀 h1 인코더가 필요했고, 이는 L+ 노력 + Chrome-rot 유지보수 + "Zero C deps" 포지셔닝 훼손이 한 곳에 겹친 가장 비싸고 부서지기 쉬운 경로. 유지보수자 결정으로 **wreq 채택** — reqwest 하드 포크라 API 호환, Chrome 149 프로파일 내장(`wreq_util::Emulation::Chrome149`).

**검증 (`cargo run -- fetch https://tls.peet.ws/api/all`, 2026-06-25)**:
- `http_version: h2` (이전 HTTP/1.1).
- TLS: GREASE cipher 선행, Chrome cipher suites, **ECH(65037)**, `compress_certificate`/brotli, ALPS(17613), X25519MLKEM768. `ja3_hash: df1f9f9fd264069b28f56b25808eb840`, `ja4: t13d1516h2_8daaf6152771_d8a2da3f94cd`.
- HTTP/2 `akamai_fingerprint: 1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p` = Chrome SETTINGS + 의사헤더 순서(`:method :authority :scheme :path`).
- `sec-ch-ua: "Google Chrome";v="149"` + 일치하는 Chrome 149 UA.

## 2. 핵심 원리 (유효): 4차원 교차 레이어 일치

탐지의 주 신호는 *불일치* — 4 차원이 동일 Chrome에 합의해야 한다. 이 원리는 그대로. 다만 이제 **wreq가 3/4 차원(TLS·H2·헤더)을 한 번에** 담당:

| 차원 | 핑거프린트 | 담당 | 상태 |
|---|---|---|---|
| TLS ClientHello | JA3 / JA4 | wreq / BoringSSL | ✓ Chrome 149 검증 |
| HTTP/2 프레임 | Akamai H2 | wreq | ✓ 검증 |
| HTTP 헤더 | JA4H (순서+케이스) | wreq | ✓ 검증 (Title-Case + Chrome 순서) |
| JS 표면 | 환경 탐침 | boa + `js/stealth.rs` | △ 수제 (P2.2) |

`Emulation::Chrome149`가 단일 진실 소스로 세 전송 차원을 구동. **주의**: emulation 기본 헤더(UA·sec-ch-ua)와 `config.user_agent`는 버전(149)·플랫폼(macOS)이 일치해야 — 어긋나면 즉시 교차-레이어 신호가 된다.

## 3. wreq 통합 (완료)

- `network/client.rs`: `reqwest` → `wreq`, 빌더에 `.emulation(Emulation::Chrome149)` **선행**(wreq 규칙: emulation 후 H1/H2/TLS 미세조정).
- `config.user_agent` 기본값 = Chrome 149 macOS UA → 전송·JS 양측 합의.
- API 차이점 해결: `Response::uri()`(=`&Uri`, `url()` 아님), `Attempt.uri`(필드), `tls_cert_verification(false)`(=`danger_accept_invalid_certs`), `form` 피처.
- 빌드 의존: BoringSSL(cmake·go·perl·libclang). macOS arm64 빌드 검증 완료.
- CLI 검색엔진/테스트는 기존 `reqwest` 유지(전송 스텔스와 무관).

## 4. 남은 수제 영역

### 4.1 JS 표면 — boa↔V8 동일성 (P2.2)
wreq는 전송만 고친다. JS 실행 환경은 여전히 boa. CreepJS/FP-Collect 급 정밀 탐지가 노리는 차이 좁히기: `Function.prototype.toString` 네이티브 표기(`function name() { [native code] }`), `Error.stack` V8 포맷, 누락 글로벌(`Proxy`/`WeakRef`/`FinalizationRegistry`/`Intl`), `navigator.plugins`/`mimeTypes`. 기존 `js/stealth.rs`를 Chrome149 프로파일 구동으로 확장.

### 4.2 managed 챌린지 솔버 (P0.3)
전송이 Chrome이라 **수동 핑거프린트 티어는 통과**. managed 챌린지(Cloudflare/DataDome)는 챌린지 JS를 boa에서 실행 → `cf_clearance` 쿠키 획득 → 동일 세션 재fetch 루프. `session.rs navigate()`(현재 단일 fetch)에 감지→실행→재fetch 루프 + 403 재시도 승격 추가. 성공은 §4.1(boa가 챌린지 탐침 통과)에 좌우 → **P0.3 ∩ P2.2 동시 필요**.

## 5. 단계화 (wreq 기반)

| 단계 | 내용 | 상태 |
|---|---|---|
| Transport-0 | wreq + Chrome149 emulation + HTTP/2 + UA 일치 | ✓ 완료·검증 |
| JS-1 | boa↔V8 상위 벡터 (P2.2) | 진행 |
| Headless | SPA(P0.2)·추출(P1.1)·auto-wait(P3)·관찰자(P1.2)·웹컴포넌트(P2.1) | 진행(병렬 서브에이전트) |
| Challenge | managed 챌린지 솔버 (P0.3) | P2.2 후 |

## 6. 기각된 대안: pure-Rust rustls 포크 (기록용)

pure-Rust 제약 하의 원래 계획. 기각 이유:
- rustls는 ClientHello 확장 순서·GREASE를 내부 고정 → JA3 정합에 **포크 필수**(단순 재정렬이 아니라 확장 방출 재작성).
- Chrome이 쓰는 ECH/ALPS/compressed_certificate/delegated_credentials 중 **ECH(HPKE)는 별도의 벽**.
- HTTP/1.1 헤더 케이스는 `http::HeaderName`이 소문자 강제 → 커스텀 h1 인코더 필요.
- Chrome-rot(버전별 확장 세트 변화)에 대한 지속 유지보수.
- 생태계: pure-Rust Chrome-impersonation 기성 크레이트 부재(wreq/rquest는 BoringSSL 기반).
→ 제약 완화로 이 경로 전체 기각, wreq로 대체.
