# v0.12.x Follow-up — Per-tab Routing & Cross-project Publish

> **Status:** Design (handoff spec for oxi and oxios teams)
> **Author:** oxibrowser team
> **Scope:** oxi-agent → oxi-sdk → oxios (oxibrowser is **done**)
> **Estimated effort:** oxi-agent ~200 LoC, oxi-sdk compile fix (pre-existing) ~50 LoC, oxios dep bump + publish ~30 LoC. About 1–2 days.

---

## 0. TL;DR — what's done, what's left

The v0.12 observability initiative (`docs/designs/2026-06-04-oxibrowser-observability.md`) is **complete on the oxibrowser side**. Two crates shipped:

| Version | Commit | What it added |
|--------:|--------|---------------|
| `oxibrowser-core 0.12.0` | `493f419` | `BrowserEvent` enum (4 variants), `Browser::subscribe_events()`, `Tab::emit()` on every `goto` / `wait_for` / `screenshot`. |
| `oxibrowser-core 0.12.1` | `2222a86` | `tab_id: Uuid` field on every `BrowserEvent` variant. Required in Rust, `#[serde(default = "Uuid::nil")]` for wire backwards compat. New `Tab::tab_id()` getter. |

What remains lives in three other projects and is described in this document:

1. **oxi-agent** — replace the single-slot `ProgressForwarder` with per-`tab_id` routing. Stop sending `tab_id: None`.
2. **oxi-sdk** — fix a pre-existing 5-error compile break (unrelated to this work) so v0.27.1 can ship.
3. **oxios** — bump `oxi-sdk = "0.26.2"` → `"0.27"` and re-publish `oxios-kernel` and `oxios-web` at v1.0.4.

`oxibrowser` itself needs **no further work** for this initiative. The 0.12.x line is the stable contract; consumers can depend on `oxibrowser-core = "0.12"` and get the right behaviour.

---

## 1. The wire contract oxibrowser exposes (frozen)

This is the public surface that oxi-agent and oxios rely on. It is **stable** — no changes are planned in the 0.12.x line. If you need to evolve it, that's a v0.13 RFC.

### 1.1 `oxibrowser_core::event::BrowserEvent`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrowserEvent {
    NavigationStarted {
        tab_id: Uuid,                    // <-- required, v0.12.1+
        url: String,
    },
    WaitingForSelector {
        tab_id: Uuid,
        selector: String,
        timeout_ms: u64,
    },
    DocumentReady {
        tab_id: Uuid,
        final_url: String,
        title: String,
        status: u16,
        total_bytes: u64,                // <-- see §1.4 caveat
        js_script_count: usize,          // <-- see §1.4 caveat
        total_duration: Duration,
    },
    ScreenshotCaptured {
        tab_id: Uuid,
        bytes: usize,
        viewport_width: u32,
        duration: Duration,
    },
}
```

**Stability rules:**

- The enum is `#[non_exhaustive]`. **Adding** a new variant is backwards-compatible at the source level but breaks every downstream `match` that doesn't include a wildcard arm. Treat the current 4 variants as a hard contract for v0.12.x.
- Adding a **field** to an existing variant is a breaking change. Do not do it in 0.12.x.
- The `tab_id` field is `#[serde(default = "Uuid::nil")]` on each variant. JSON payloads that omit `tab_id` (e.g. from older 0.12.0 clients) deserialize with `tab_id = Uuid::nil()`. Use this to detect "no tab info" if you need to (the `nil` UUID is a sentinel, not a real tab).

### 1.2 `Browser::subscribe_events()`

```rust
pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<BrowserEvent>
```

- 32-slot buffer, oldest-dropped on overflow.
- Multiple subscribers are supported; each gets its own queue.
- The receiver returns `RecvError::Lagged(n)` when the consumer falls behind. The number `n` is the count of events dropped. **Log and continue** — do not treat `Lagged` as fatal.
- The receiver returns `RecvError::Closed` when the `Browser` is dropped. Use this for clean shutdown of drain tasks.

### 1.3 `Tab::tab_id()`

```rust
impl Tab {
    pub fn tab_id(&self) -> Uuid { ... }
}
```

- Stable for the lifetime of a `Tab` (clones share the same id).
- `Uuid::nil()` for tabs created via the test-only `Tab::new()` path; `Uuid::new_v4()` for tabs created via `Browser::new_tab()`.

### 1.4 Known minor inaccuracies in the v0.12.0 docs (no fix planned)

These don't affect wire compatibility. They are worth a follow-up doc-comment patch in a future v0.12.x point release if anyone cares:

- `DocumentReady.total_bytes` is the size of the **post-parse, re-serialized HTML body** (`result.html.len() as u64`), not the wire `Content-Length`. The doc comment claims the latter.
- `DocumentReady.js_script_count` is the count of **`<script>` resources extracted from the DOM** (`page.root_frame().extract_resource_urls().filter(kind == Script).count()`), not the count of scripts the JS runtime actually executed. A script with a 404'd `src` or a `defer` script that hasn't fired yet is still counted.

These are visible-quality issues, not correctness bugs. Leave them for v0.12.2+ unless they cause real confusion.

### 1.5 Versioning

- `oxibrowser-core = "0.12"` constraint resolves to 0.12.1 (and any future 0.12.x). No action needed in downstream Cargo.toml to pick up 0.12.1.
- oxibrowser 0.13.0 would be the next chance to evolve the contract. The `non_exhaustive` marker means a 0.12.x → 0.13.0 bump is required to make any structural change.

---

## 2. oxi-agent — deferred per-tab routing

> **Project**: oxi (at `~/code/oxi/`, branch `main`, currently at `6d3f5ac`)
> **Tracking issue / PR**: open a new issue titled "Replace ProgressForwarder with per-`tab_id` routing" and link to this design.
> **Estimated effort**: ~200 LoC + tests. About 1 day.

### 2.1 What shipped in v0.27.2 (and why it's not enough)

The previous PR (`09f0176 feat(agent): add tab_id to AgentEvent::ToolExecutionUpdate`) added the field on the agent-loop event. The construction site currently does:

```rust
emit_clone(AgentEvent::ToolExecutionUpdate {
    tool_call_id: tool_call_id_clone.clone(),
    tool_name: tool_name.clone(),
    partial_result: msg,
    tab_id: None,           // <-- always None, see below
});
```

The reason `tab_id` is hard-coded `None` is that the `ProgressCallback` signature in `oxi-agent/src/tools.rs:138` is:

```rust
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;
```

The callback receives only the **short label string**, not the structured `BrowserEvent`. The agent loop has no way to recover the `tab_id` at emit time.

To preserve the simple `Fn(String)` API, the previous PR also added a `SequentialOnly` execution mode on `BrowseTool` (`4c1c7e1 fix(agent): force BrowseTool to run sequentially`). This is **defense-in-depth**, not a real fix: it serialises tool calls but doesn't address the underlying single-slot forwarder.

### 2.2 What needs to change

The proper fix is in three steps. **Read all three before starting** — they interact.

#### Step A — change the callback signature

`oxi-agent/src/tools.rs:138`:

```rust
// OLD
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;

// NEW
pub type ProgressCallback = Arc<dyn Fn(oxibrowser_core::BrowserEvent) + Send + Sync>;
```

This is the only place in `oxi-agent` that needs the type alias updated. All `set(cb)` / `clear()` / `invoke()` calls go through `ProgressForwarder` and now receive the structured event.

#### Step B — replace the single-slot `ProgressForwarder` with a per-`tab_id` registry

`oxi-agent/src/tools/browse/engine.rs` — replace the current `ProgressForwarder` (which holds `Mutex<Option<ProgressCallback>>`) with a `TabCallbackRegistry` that holds `Mutex<HashMap<Uuid, ProgressCallback>>`:

```rust
pub struct TabCallbackRegistry {
    callbacks: parking_lot::Mutex<HashMap<Uuid, ProgressCallback>>,
}

impl TabCallbackRegistry {
    pub fn new() -> Self { Self { callbacks: Mutex::new(HashMap::new()) } }
    pub fn set(&self, tab_id: Uuid, cb: ProgressCallback) {
        self.callbacks.lock().insert(tab_id, cb);
    }
    pub fn clear(&self, tab_id: &Uuid) {
        self.callbacks.lock().remove(tab_id);
    }
    pub fn invoke(&self, tab_id: &Uuid, event: oxibrowser_core::BrowserEvent) {
        if let Some(cb) = self.callbacks.lock().get(tab_id).cloned() {
            cb(event);
        }
    }
    pub fn is_set(&self, tab_id: &Uuid) -> bool {
        self.callbacks.lock().contains_key(tab_id)
    }
    pub fn len(&self) -> usize { self.callbacks.lock().len() }
}
```

Update the `BrowserEngine` trait's default `progress_forwarder` method to return a fresh empty registry instead of a fresh empty forwarder.

#### Step C — wire OxiBrowserEngine to route by `tab_id`

`oxi-agent/src/tools/browse/oxibrowser_backend.rs`:

The background drain task currently does:

```rust
progress_clone.invoke(event.short_label());
```

Change it to route by `event.tab_id`:

```rust
progress_clone.invoke(&event.tab_id, event);
```

Add a `tab_id: Uuid` field and a `registry: Arc<TabCallbackRegistry>` field to `OxiTab`. The `BrowserEngine::new_tab` impl passes the engine's registry into the new `OxiTab::new(inner, config, registry)`. The `OxiTab`'s `tab_id` is obtained from `inner.tab_id()` (the new getter on `oxibrowser_core::Tab`, §1.3).

Add `OxiTab::set_progress_callback(cb: ProgressCallback)` which calls `self.registry.set(self.tab_id, cb)`, and `OxiTab::clear_progress_callback()` which calls `self.registry.clear(&self.tab_id)`. The latter should be called when the tab closes (in `OxiTab::close` or via `TabGuard`).

#### Step D — update `BrowseTool` to register per-tab

`oxi-agent/src/tools/browse/browse_tool.rs`:

`on_progress` is called by the agent loop **before** `execute`. At that point the `OxiTab` doesn't exist yet. Store the callback on the tool instance; in `execute`, when you open a new tab, register the callback on the tab. `TabGuard` (or the manual `guard.close().await` at the end of `execute`) clears the callback.

```rust
pub struct BrowseTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    pending_callback: parking_lot::Mutex<Option<ProgressCallback>>,
}

impl AgentTool for BrowseTool {
    fn on_progress(&self, callback: ProgressCallback) {
        *self.pending_callback.lock() = Some(callback);
    }

    async fn execute(&self, ...) -> Result<AgentToolResult, ToolError> {
        let raw_tab = self.engine.new_tab().await?;
        let cb_opt = self.pending_callback.lock().take();
        if let Some(cb) = cb_opt {
            raw_tab.set_progress_callback(cb);
        }
        let guard = TabGuard::new(raw_tab);
        // ... existing body unchanged ...
        guard.close().await;   // <-- TabGuard::close should call clear_progress_callback
    }
}
```

#### Step E — populate `tab_id` on `AgentEvent::ToolExecutionUpdate`

`oxi-agent/src/agent_loop/tool_exec.rs:441`:

Now that the callback receives the full `BrowserEvent`, capture `tab_id` in the closure:

```rust
let progress_cb: Arc<dyn Fn(oxibrowser_core::BrowserEvent) + Send + Sync> =
    Arc::new(move |event: oxibrowser_core::BrowserEvent| {
        emit_clone(AgentEvent::ToolExecutionUpdate {
            tool_call_id: tool_call_id_clone.clone(),
            tool_name: tool_name.clone(),
            partial_result: event.short_label(),
            tab_id: Some(event.tab_id),
        });
    });
tool.on_progress(progress_cb);
```

#### Step F — remove the `SequentialOnly` workaround

Now that per-tab routing prevents the race, `BrowseTool::execution_mode` can return `ParallelSafe` again. But: **only do this after the per-tab routing is verified** with a concurrent stress test (two `BrowseTool::execute` calls overlapping, two tabs, navigate, assert each callback fires only for its own tab's events). If you leave `SequentialOnly` as defense-in-depth, document why.

**Recommended**: keep `SequentialOnly` for now. The right time to lift it is when there's a concrete use case (multi-tab scraping) that needs parallel tabs. The race is gone either way.

### 2.3 Tests to add

In `oxi-agent/src/tools/browse/engine.rs` (unit):

1. `tab_callback_registry_set_and_invoke` — register cb for tab A, invoke for A → fires; invoke for B → no fire.
2. `tab_callback_registry_set_replaces_per_tab` — register cb_A for A and cb_B for B, invoke for A → only A fires.
3. `tab_callback_registry_clear` — clear A, invoke for A → no fire.
4. `tab_callback_registry_default_is_empty` — `new()` has 0 callbacks.

In `oxi-agent/src/tools/browse/oxibrowser_backend.rs` (integration):

5. `engine_routes_events_by_tab_id_concurrent` — open two tabs in one engine, register two callbacks, navigate each. Assert each callback only fires for its own tab's events.

In `oxi-agent/src/agent_loop/tool_exec.rs` (or wherever the test for `ToolExecutionUpdate` lives):

6. Verify `tab_id` is populated when a callback fires.

### 2.4 Local test infrastructure

`oxibrowser-core 0.12.1` is not yet on crates.io. While testing locally, add a temporary `[patch.crates-io]` to the oxi workspace's root `Cargo.toml`:

```toml
[patch.crates-io]
oxibrowser-core = { path = "/path/to/oxibrowser/crates/oxibrowser-core" }
```

**Remove this before commit.** The commit should be clean — no temporary patches.

### 2.5 Versioning

- `oxi-agent`: 0.27.2 → 0.27.3 (the callback signature change is a breaking change for any direct consumer of `oxi_agent::ProgressCallback`; bump minor as a courtesy signal)
- `oxi-sdk`: 0.27.1 → 0.27.2 (re-export only, but bump to match)

### 2.6 CHANGELOG entry

Under a new `## [0.27.3] - YYYY-MM-DD` section (or under `[Unreleased]` if you prefer to batch):

- `ProgressForwarder` replaced by per-`tab_id` `TabCallbackRegistry`. Concurrent `BrowseTool` calls no longer race.
- `ProgressCallback` signature changed from `Fn(String)` to `Fn(oxibrowser_core::BrowserEvent)`. This is a breaking change for direct consumers of the type alias.
- `AgentEvent::ToolExecutionUpdate.tab_id` is now populated (no longer always `None`).
- `BrowseTool::execution_mode` may be returned to `ParallelSafe` (or kept as `SequentialOnly` as defense-in-depth — your call, document it).

---

## 3. oxi-sdk — unblock the publish

> **Project**: oxi (at `~/code/oxi/`, branch `main`)
> **Tracking**: pre-existing issue, NOT introduced by the v0.12 work. Has been failing on `main` since before this initiative.
> **Estimated effort**: ~50 LoC. About 2 hours.

### 3.1 The breakage

`cargo publish --dry-run -p oxi-sdk` fails with 5 errors:

```
error[E0053]: method `resolve_provider` has an incompatible type for trait
error[E0053]: method `resolve_model` has an incompatible type for trait
error[E0053]: method `resolve_provider` has an incompatible type for trait
error[E0053]: method `resolve_model` has an incompatible type for trait
error[E0308]: mismatched types
```

`oxi-sdk` is in the same workspace as `oxi-agent` and presumably compiles fine via `cargo build --workspace` because the resolver uses the local path. The publish dry run uses the *tarball* (the `Cargo.toml` of the published crate + its declared `oxi-agent = "0.27.x"` dependency), and the resolved version of `oxi-agent` from crates.io has a different `resolve_provider` / `resolve_model` signature than the local source.

### 3.2 Root cause (to be confirmed by the oxi team)

Two possibilities, in order of likelihood:

1. **`oxi-sdk` was published with `oxi-agent = "0.27"`** but the latest `oxi-agent` on crates.io has a different method signature than the local source. The local source has a `pub fn` while the published version had a different shape. Fix: align the local source to the published API (or vice versa).
2. **`oxi-sdk`'s `Cargo.toml` declares a more permissive version range** than what's actually used locally. Tightening the range to `"=0.27.2"` would force the resolver to pick the local version.

The oxi team should run `git log --oneline -- oxi-sdk/src/ports/` and `oxi-agent/src/agent/ports/` to find the last commit that touched these signatures and see which side is the source of truth.

### 3.3 Verification

Until this is fixed, `oxi-sdk 0.27.1` cannot be published, which blocks the oxios v1.0.4 publish chain (see §5). Resolve it before continuing the publish flow.

---

## 4. oxios — bump the oxi-sdk dependency

> **Project**: oxios (at `~/code/oxios/`, branch `main`, currently at `b03baec`)
> **Tracking**: create a new issue titled "Bump oxi-sdk to 0.27 for tab_id propagation" and link here.
> **Estimated effort**: ~30 LoC (Cargo.toml + minor adapter changes). About 2 hours.

### 4.1 What shipped in v1.0.4

Three commits are already on `main` (and a fourth from the tsc cleanup):

| Commit | Subject |
|--------|---------|
| `7ef6d4c` | `feat(kernel): propagate tab_id through KernelEvent::ToolExecutionProgress` |
| `c66f764` | `feat(web): include tab_id in tool_progress WS chunk and SSE event` |
| `b03baec` | `feat(web): show tab-id badge in ActivityCard; finish tsc cleanup` |
| `56938a1` | `fix(web): clear 96 pre-existing tsc errors` (already shipped) |

The kernel and web code already constructs `KernelEvent::ToolExecutionProgress { ..., tab_id }` from `AgentEvent::ToolExecutionUpdate { tab_id, ... }`. The `tab_id` field is `Option<Uuid>` and uses `#[serde(default, skip_serializing_if = "Option::is_none")]` so the wire format is backwards-compatible with older oxi-agent versions.

### 4.2 What needs to change in oxios

One Cargo.toml line:

```toml
# In oxios root Cargo.toml, find the existing line:
oxi-sdk = "0.26.2"

# Change to:
oxi-sdk = "0.27"
```

The `0.27` constraint resolves to `0.27.x` for any `x ≥ 2`, which gives oxios access to the `tab_id` field on `AgentEvent::ToolExecutionUpdate`. After this bump, the `agent_runtime.rs` arm added in `7ef6d4c` will compile against the new field and start reading the (currently `None`) value.

Once the oxi-agent per-tab routing PR (§2) lands and oxi-sdk 0.27.2 is published, the `tab_id` will start being populated end-to-end. **No oxios code change is needed for that** — the propagation already works.

### 4.3 Verification

After the bump:

```bash
cd /Volumes/MERCURY/PROJECTS/oxios
cargo test -p oxios-kernel
cargo test -p oxios-web
cd surface/oxios-web/web
bun test
bunx tsc --noEmit
```

All four should still pass. (The web build needs `web/dist/` to exist — `bun run build` first, or the rust-embed derive in `surface/oxios-web/src/plugin.rs` will fail.)

### 4.4 Bump the workspace CHANGELOG

Add a bullet to the existing `[Unreleased]` section in `oxios/CHANGELOG.md`:

- `oxi-sdk = "0.27"` (was `0.26.2`). Picks up the new `tab_id` field on `AgentEvent::ToolExecutionUpdate`. No code change in oxios; the field is `#[serde(default)]` so older oxi-agent versions keep working.

---

## 5. Publish order

The full chain, after the §2 and §3 work lands:

```
1. oxibrowser-core 0.12.1      (already shipped to git, just needs publish)
2. oxi-sdk 0.27.2              (after §3 compile fix)
3. oxi-agent 0.27.3            (after §2 per-tab routing)
4. oxios — workspace dep bump to oxi-sdk 0.27    (after step 3)
5. oxios-kernel 1.0.4
6. oxios-web 1.0.4
```

Each step requires the previous step's crate to be on crates.io before the next can resolve. Plan for **6 separate publish operations**, each gated on the previous.

### 5.1 Pre-publish checklist for each crate

```bash
# 1. oxibrowser-core (oxibrowser repo, branch main)
cd /Volumes/MERCURY/PROJECTS/oxibrowser
cargo test -p oxibrowser-core      # must be green
cargo publish --dry-run -p oxibrowser-core --allow-dirty  # verify packaging
cargo publish -p oxibrowser-core   # actual upload
```

```bash
# 2-3. oxi-sdk first, then oxi-agent (oxi repo, branch main, after §2 + §3 land)
cd /Volumes/MERCURY/PROJECTS/oxi
cargo test -p oxi-sdk --features native-browser 2>/dev/null
cargo test -p oxi-agent --features native-browser
cargo publish -p oxi-sdk            # BEFORE oxi-agent (oxi-sdk depends on oxi-agent)
cargo publish -p oxi-agent
```

```bash
# 4-6. oxios (oxios repo, branch main, after §4 dep bump)
cd /Volumes/MERCURY/PROJECTS/oxios
cargo test -p oxios-kernel
cargo test -p oxios-web
cd surface/oxios-web/web && bun run build && cd ../../..
cargo publish -p oxios-kernel
cargo publish -p oxios-web
```

### 5.2 Known pre-existing issues to be aware of

These are NOT caused by the v0.12 work and don't block publish, but will surface during testing:

- **oxibrowser**: 4 unrelated dirty files in `main` (`README.md`, `frame.rs`, `page.rs`, `session.rs`). Use `--allow-dirty` for the publish, or commit them first.
- **oxi**: workspace dirty in `main` (oxibrowser-side untracked `oxi-progress-fix-report.md` was cleaned up; the current dirty state from the user's previous session is unrelated). Use `--allow-dirty` or commit.
- **oxios**: 10 pre-existing test failures in `browse_script_tool` (YAML parsing), unrelated to this work. They fail on `main` and on the v0.12 branch.
- **oxios-web frontend**: `web/dist/` must exist for `cargo build -p oxios-web` to succeed (rust-embed requirement). `bun run build` first.

---

## 6. Open questions for the oxi and oxios teams

These aren't blocking, but the answers shape the v0.13 RFC. Defer to follow-up issues:

1. **Lift `SequentialOnly` on `BrowseTool`?** Now that per-tab routing prevents the race, parallel `BrowseTool::execute` calls would be safe. The oxi team should decide based on real multi-tab scraping use cases. Don't lift speculatively.

2. **Persist `ToolExecutionProgress` to the session log?** Currently ephemeral (the kernel event is published on the bus, but not stored). The trajectory step schema would need a new `progress_fragments: Vec<String>` field. Probably defer to v0.13.

3. **Backward compat for `ChatActivity.isRunning`?** Adding `isRunning?: boolean` to the `tool_call` variant is technically additive, but downstream consumers (mobile app, JSON export) that match on the variant may need updates. Worth a coordination call before publishing oxios-web 1.0.4.

4. **oxi-sdk re-exports of `BrowserEvent` and `tab_id`-related types?** Currently `oxi-sdk` only re-exports `AgentEvent` and `BrowserEvent`. If a downstream consumer wants to construct `BrowserEvent` themselves (e.g. a custom `BrowserEngine` impl), they may want access to the `tab_id` field's type. Currently they get it transitively via `oxibrowser_core`. Document this dependency clearly.

---

## 7. File / commit index for reference

| Repo | Branch | Commits added in this initiative |
|------|--------|---------------------------------|
| oxibrowser | `main` (at `2222a86`) | `493f419` v0.12.0, `66a06c1` CHANGELOG, `2222a86` v0.12.1 |
| oxi | `main` (at `6d3f5ac`) | `4c1c7e1` SequentialOnly, `09f0176` tab_id field |
| oxios | `main` (at `b03baec`) | `7ef6d4c` kernel, `c66f764` web backend, `b03baec` web frontend + tsc, `56938a1` tsc fix (96→0) |

| Worktree created in this session | Status |
|---------------------------------|--------|
| `/Volumes/MERCURY/PROJECTS/oxibrowser-tab-id` | merged, removed |
| `/Volumes/MERCURY/PROJECTS/oxios-tsc-fix` | merged, removed |
| `/Volumes/MERCURY/PROJECTS/oxios-tool-progress` | merged, removed |
| `/Volumes/MERCURY/PROJECTS/oxios-tab-id-propagation` | merged, removed |
| `/Volumes/MERCURY/PROJECTS/oxi-per-tab-forwarder` | merged (partial — only the tab_id field landed; the per-tab registry refactor is §2 of this doc), removed |
| `/Volumes/MERCURY/PROJECTS/oxi-progress-fix` | merged, removed |

---

## 8. Contact

- **oxibrowser maintainer**: see `CODEOWNERS` in the oxibrowser repo
- **oxi maintainer**: a7garden (`oxi/CODEOWNERS`)
- **oxios maintainer**: see `oxios/CODEOWNERS`

For the v0.12 observability initiative, file issues in the respective repos and link to this design document. The wire contract (§1) is frozen for the 0.12.x line — any proposed change is a v0.13 RFC.
