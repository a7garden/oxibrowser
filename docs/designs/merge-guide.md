# 병합 가이드: Session A + Session B → main

> 두 병렬 세션이 완료된 후 main으로 병합하는 순서.

## 브랜치 구조

```
main (b94dd2c)
  ├── feat/web-platform  ← Session A: runtime.rs, session.rs, frame.rs 수정
  │     P0.1 Error Recovery
  │     P2.1 JS Globals (randomUUID, queueMicrotask, self)
  │     P2.2 History API (pushState, back, forward, go)
  │     P2.3 Location API (assign, replace, reload, search, hash)
  │     P2.4 Event System (Event ctor, DOMContentLoaded/load)
  │
  └── feat/cdp-perf      ← Session B: CDP files, Cargo.toml, benches 수정
        P0.4 Fetch Interception (requestPaused 연동)
        P1.1 captureScreenshot text mode
        P3.1 Binary Size optimization
        P3.2 Startup Benchmark
        P3.3 Memory Benchmark
```

## 파일 충돌 분석

| 파일 | Session A | Session B | 충돌? |
|------|-----------|-----------|--------|
| `core/src/js/runtime.rs` | ✏️ 수정 | ❌ 금지 | 없음 |
| `core/src/session.rs` | ✏️ 수정 | ❌ 금지 | 없음 |
| `core/src/frame.rs` | ✏️ 수정 | ❌ 금지 | 없음 |
| `cdp/src/domains/fetch.rs` | ❌ 금지 | ✏️ 수정 | 없음 |
| `cdp/src/domains/page.rs` | ❌ 금지 | ✏️ 수정 | 없음 |
| `cdp/src/domains/mod.rs` | ❌ 금지 | ✏️ 수정 | 없음 |
| `cdp/src/event.rs` | ❌ 금지 | ✏️ 수정 | 없음 |
| `Cargo.toml` (root) | ❌ 금지 | ✏️ 수정 | 없음 |
| `core/benches/core_bench.rs` | ❌ 금지 | ✏️ 수정 | 없음 |
| `Cargo.lock` | 자동 | 자동 | **가능** |

**결론**: 파일 충돌이 거의 없음. `Cargo.lock`만 자동 병합으로 해결.

## 병합 순서

```bash
# 1. main에서 feat/web-platform 머지 (A 먼저)
git checkout main
git merge feat/web-platform --no-ff -m "feat: v0.5 web platform (P0.1 + P2.1-4)"

# 2. 빌드 + 테스트 확인
cargo build --workspace
cargo test --workspace

# 3. feat/cdp-perf 머지
git merge feat/cdp-perf --no-ff -m "feat: v0.5 CDP + performance (P0.4 + P1.1 + P3)"

# 4. 빌드 + 테스트 확인
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 5. Release 빌드
cargo build --release
ls -lh target/release/oxibrowser

# 6. 벤치마크 실행
cargo bench --bench core_bench

# 7. 최종 커밋 + 푸시
git push origin main

# 8. 브랜치 정리
git branch -d feat/web-platform feat/cdp-perf
```

## Cargo.lock 충돌 시

```bash
# 자동 재생성
git checkout --theirs Cargo.lock
cargo update -p oxibrowser-core
git add Cargo.lock
git merge --continue
```

## 완료 기준

- [ ] `cargo test --workspace` — 280+ tests pass
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — 0 warnings
- [ ] `cargo build --release` — binary < 10MB
- [ ] `cargo bench` — browser_startup < 50ms
- [ ] E2E: Fetch.requestPaused 이벤트 수신
- [ ] E2E: Page.captureScreenshot(format: "text") 반환
- [ ] Unit: history.pushState/back/forward 동작
- [ ] Unit: location.assign/replace/reload 동작
- [ ] Unit: Event constructor + dispatchEvent with target
- [ ] Unit: crypto.randomUUID() + queueMicrotask()
