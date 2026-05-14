# 병렬 실행 + 병합 가이드

> Session A와 Session B를 git worktree로 동시 실행 후 main으로 병합.

## 1단계: 워크트리 생성 (메인 세션에서 실행)

```bash
cd /Volumes/MERCURY/PROJECTS/oxibrowser

# 두 워크트리를 동시에 생성
git worktree add ../session-a-web-platform -b feat/web-platform 4f5f303
git worktree add ../session-b-cdp-perf     -b feat/cdp-perf     4f5f303

# 확인
git worktree list
# /Volumes/MERCURY/PROJECTS/oxibrowser              4f5f303 [main]
# /Volumes/MERCURY/PROJECTS/session-a-web-platform   4f5f303 [feat/web-platform]
# /Volumes/MERCURY/PROJECTS/session-b-cdp-perf       4f5f303 [feat/cdp-perf]
```

## 2단계: 두 에이전트에 프롬프트 전달

### 에이전트 A — cwd를 워크트리로 설정
```
cwd: /Volumes/MERCURY/PROJECTS/session-a-web-platform
prompt: docs/designs/session-a-web-platform.md 파일을 읽고 따라라.
        모든 작업은 이 워크트리 디렉토리에서 수행한다.
```

### 에이전트 B — cwd를 워크트리로 설정
```
cwd: /Volumes/MERCURY/PROJECTS/session-b-cdp-perf
prompt: docs/designs/session-b-cdp-perf.md 파일을 읽고 따라라.
        모든 작업은 이 워크트리 디렉토리에서 수행한다.
```

## 3단계: 병렬 실행

두 에이전트가 완전히 독립된 파일시스템에서 동시 작업:

```
/Volumes/MERCURY/PROJECTS/session-a-web-platform/   ← 에이전트 A
├── crates/oxibrowser-core/src/js/runtime.rs         ✏️ 수정
├── crates/oxibrowser-core/src/session.rs             ✏️ 수정
└── crates/oxibrowser-core/src/frame.rs               ✏️ 수정

/Volumes/MERCURY/PROJECTS/session-b-cdp-perf/        ← 에이전트 B
├── crates/oxibrowser-cdp/src/domains/fetch.rs        ✏️ 수정
├── crates/oxibrowser-cdp/src/domains/page.rs         ✏️ 수정
├── crates/oxibrowser-cdp/src/domains/mod.rs          ✏️ 수정
├── crates/oxibrowser-cdp/src/event.rs                ✏️ 수정
├── Cargo.toml (root)                                 ✏️ 수정
└── crates/oxibrowser-core/benches/core_bench.rs      ✏️ 수정
```

**파일 소유권이 완전히 분리 → 충돌 0%**

## 4단계: 병합 (메인 세션에서 실행)

```bash
cd /Volumes/MERCURY/PROJECTS/oxibrowser

# A 먼저 병합
git merge feat/web-platform --no-ff -m "feat: v0.5 web platform (P0.1 + P2.1-4)"
cargo test --workspace

# B 병합
git merge feat/cdp-perf --no-ff -m "feat: v0.5 CDP + performance (P0.4 + P1.1 + P3)"
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Release 빌드 확인
cargo build --release
ls -lh target/release/oxibrowser

# 벤치마크
cargo bench --bench core_bench

# 푸시
git push origin main
```

## 5단계: 워크트리 정리

```bash
git worktree remove ../session-a-web-platform
git worktree remove ../session-b-cdp-perf
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
