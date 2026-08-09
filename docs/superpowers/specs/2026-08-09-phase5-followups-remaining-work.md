# Remaining Work — Phase 4+5 Follow-ups (post-completion)

> **Context:** `docs/superpowers/specs/2026-08-09-phase5-followups-handoff.md` is
> **complete** — Sub-session A (CoreEvent sink + 7 emitters) and Sub-session B
> (lifecycle callbacks + geometry methods + ShadowRoot slot composition) all
> shipped, verified (commits `f23ee80`→`736754b`), gates green.
>
> This doc lists what is **genuinely left**, scoped precisely so the next session
> doesn't redo finished work or chase closed risks. Branch: `main`.

---

## 1. Primary remaining: screenshot rasterization of shadow content

**What:** `Page.captureScreenshot` / CLI screenshots (`capture_png`) do **not**
reflect Shadow DOM composition. Slotted content is invisible in screenshots.

**Why:** `capture_png` renders through `RenderDocument::capture_png` → Blitz's
`BaseDocument`, an **external** crates.io crate (`blitz-dom = "0.3.0-beta.1"`
— registry-sourced, not a workspace member). Blitz models a single flat tree
with no shadow/host/slot concept. Composition was implemented only at the
**DomSnapshot** level (`compose_shadow_trees`), which Blitz doesn't read.

**Contrast (already working):** every DomSnapshot-backed path DOES reflect
composition — `DOM.*` queries, `getBoxModel`/`getContentQuads`/`getNodeForLocation`,
`OXI.*`, `extract`, accessibility, `LayoutEngine`, `render_box_model_png`.
Verified end-to-end (probe4: `DOM.querySelector('#slotted')` returns the slotted node).

**Options to close it (project-level decision, not a contained patch):**
1. **Fork/patch Blitz** — vendor `blitz-dom` as a path dep and teach
   `BaseDocument` (and its Stylo style/layout) about shadow trees + slot
   distribution. Large; foreign codebase (Stylo).
2. **Compose-then-feed Blitz** — build the flattened tree (already produced for
   DomSnapshot) and write it back into a fresh `BaseDocument` before rasterizing.
   Meaning: serialize the composed `DomSnapshot` to HTML, re-parse into a new
   `RenderDocument`, and `capture_png` that. Lossy (loses listeners/styles not in
   the snapshot) but no Blitz fork. **Most tractable** — investigate first.
3. **Shadow-aware renderer** — replace Blitz. Out of scope.

**Anchor:** `crates/oxibrowser-render/src/document.rs` (`capture_png`,
`RenderDocument` wraps `blitz_dom::BaseDocument`); the composed tree is in
`DomSnapshot` (`crates/oxibrowser-core/src/js/dom_snapshot.rs`).

---

## 2. v1 refinements (accepted/deferred during the sink work)

### 2.1 Richer `Runtime.consoleAPICalled` / `exceptionThrown` serialization
- **Now:** console args are stringified in `console_fn` and sent as
  `{type:"string", value}` RemoteObjects; exception `stackTrace.callFrames` is a
  single empty frame (no real line/col).
- **Refinement:** serialize objects/numbers/arrays with proper CDP `RemoteObject`
  types; capture real JS stack frames for exceptions (boa exposes source locations).
- **Anchor:** `crates/oxibrowser-cdp/src/core_event.rs` (`emit_console`,
  `emit_exception`); arg stringification in `console_fn!`
  (`runtime.rs` ~`:2441`).

### 2.2 CoreEvent drainer graceful shutdown (optional)
- **Now:** the drainer (`CdpSession::new`) polls `core_rx.try_recv()` + 10 ms sleep
  and exits on `Disconnected` (the sender drops when the session/JS-thread
  tears down). Works, no busy-loop, no leak in practice.
- **Refinement:** add an explicit shutdown oneshot (mirrors `handle_fetch_requests`)
  for prompt deterministic exit instead of relying on disconnect.
- **Anchor:** `crates/oxibrowser-cdp/src/session.rs` drainer `tokio::spawn` (~`:84`).

---

## 3. Shadow DOM follow-ups (cheap now that the registry exists)

The `SHADOW_ROOTS` registry + compose pass are in place; these add real DOM
surface on top:

- **`HTMLSlotElement` / assignment query APIs** — `slot.assignedNodes()`,
  `slot.assignedElements()`, `node.assignedSlot`. Derivable from the compose pass
  (it already computes assignments); expose on the native shadow-root object.
- **Closed-mode hiding** — `attachShadow({mode:'closed'})` should hide shadow
  content from `element.shadowRoot` and (optionally) the composed snapshot. The
  registry tracks `mode` only on the JS side today; thread it into compose.
- **Declarative shadow DOM** — `<template shadowrootmode="open">` parsed at
  navigate time. Needs a parser hook in `run_navigation_scripts`/`from_html` to
  auto-attach. Not in scope for the composition work.
- **`shadowRoot.innerHTML` / `append`** — the native shadow root currently exposes
  `appendChild` only; add `append`/`innerHTML` setter (parse + record children,
  mirroring `DomSnapshot::set_inner_html`).

**Anchor:** `crates/oxibrowser-core/src/js/runtime.rs` `__oxi_attach_shadow`
(`:9002`); `crates/oxibrowser-core/src/js/dom_snapshot.rs` `compose_shadow_trees`
/ `distribute_slots`.

---

## 4. Closed §5 risks (do NOT reopen)

These were marked "open" in the original handoff §5 but are **resolved** — no
work needed:

| Risk | Resolution |
|---|---|
| Dialog blocking the JS thread could deadlock the pump | ✅ Resolved: concurrent CDP dispatch (`CdpSession::run` spawns per-command + response channel) + `Page.handleJavaScriptDialog` writes the shared `DialogGate` without the session lock; evaluate/nav recvs moved to `spawn_blocking` so a blocking `alert()` never stalls the runtime. |
| `request.id` (u64) vs CDP `requestId` (string) | ✅ Resolved: formatted as `"oxi-{id}"` (fetch) / `"oxi-ws-{id}"` (ws); request/response share the id. |
| JS thread pushing CoreEvent while no sink set | ✅ Resolved: `EVENT_TX` is `Option`; `push_event` is a no-op when `None` (CLI path). |

---

## 5. Broader roadmap (referenced, not detailed here)

Out of scope for these follow-ups; tracked in
`docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md`:
- **Phase 6** — network correctness: full cookie expiry/PSL/CHIPS, CORS +
  preflight, redirect policy.
- Canvas real rasterization / WebGL (currently no-op shims — sufficient for
  automation, not for visual parity).

---

## 6. Verification baseline for the next session

```bash
cargo build --features browser --bin oxibrowser      # ALWAYS this form (see handoff §1 gotcha)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Probes (ephemeral in /tmp/oxi-probe): probe.ts (10/10), probe2.ts (10/10),
# probe3.ts (6/6 geometry), probe4.ts (shadow-DOM composition).
```

End of remaining-work handoff.
