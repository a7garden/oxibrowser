# Remaining Work — ShadowRoot / Slot Composition (DomSnapshot-level)

> **Parent:** `docs/superpowers/specs/2026-08-09-phase5-followups-handoff.md` §4 item 4.
> **Status of the rest of that handoff:** ✅ Sub-session A (CoreEvent sink + 7
> emitters) and Sub-session B (lifecycle callbacks + 3 geometry methods) are
> **done and verified** (commits `f23ee80`→`108b8f6`). This single item remains.
> **Branch:** `main`. **Last commit touching this:** `af17907` (doc only).

---

## 1. Corrected blocker scope (read this first)

A prior note (commit `af17907`, since corrected) overstated this as "blocked on
Blitz, requires forking Blitz+Stylo". That conflates two layers. The accurate
scope:

| Path | Engine | Shadow-composition implementable? |
|---|---|---|
| **All DomSnapshot-backed reads**: CDP `DOM.*`, `DOM.getBoxModel/getContentQuads/getNodeForLocation`, `OXI.*`, `extract`, `getMarkdown`, accessibility tree, `LayoutEngine::compute_rect`, `render_box_model_png` | **local** code in `oxibrowser-core` | **YES — fully local, no Blitz change.** |
| **Screenshot rasterization** `capture_png` (`Page.captureScreenshot`, CLI screenshots) | Blitz `BaseDocument` (external `blitz-dom 0.3.0-beta.1`, flat tree, no shadow concept) | Gated on Blitz — out of scope here. |

**The implementable work is the DomSnapshot-level composition.** Doing it makes
shadow content + slotted children visible to every read/layout/query path the
automation tooling actually uses (DOM queries, box models, content extraction).
Screenshot rasterization reflecting slots is a separate, Blitz-gated follow-up.

The unified live DOM the original handoff blamed **already exists**
(`RenderDocument` on the JS thread — `runtime.rs:1029`, "single source of truth
after unification"). It is not the blocker.

---

## 2. The key fact that makes this local

`DomSnapshot::from_render_document` (`crates/oxibrowser-core/src/js/dom_snapshot.rs:217`)
is **entirely local code**: it walks Blitz's tree via `collect_from_render`
(`:1141`) and builds a `DomSnapshot` (`nodes: HashMap<u32, DomNode>`, single
`root_id`) that OxiBrowser owns. Every DomSnapshot-backed consumer reads *that*
local snapshot — never Blitz directly. So if the snapshot reflects the
**composed (flattened) shadow tree**, all those consumers see slotted content
for free.

`DomNode` (`dom_snapshot.rs:55`) = `{ id, tag, attributes, text_content,
children: Vec<u32>, parent: Option<u32>, node_type: u8 }`. No shadow/host/slot
fields today.

---

## 3. Design — side shadow-tree registry + compose-on-snapshot

Keep a **side shadow-tree registry** (shadow content does NOT live in the Blitz
tree), then **compose** it into the flat snapshot when `from_render_document`
runs. Shadow DOM's rendered form *is* a flat tree (the "flattened tree"), so
Blitz never needs to know about shadow boundaries.

### 3.1 Shadow-tree registry (new, local)

The registry maps a **host node id → shadow tree**. The shadow tree is a small
`HashMap<u32, DomNode>` forest with its own ids (use a dedicated high-id range
to avoid colliding with Blitz/`NEXT_NODE_ID`, e.g. `0x4000_0000..`).

Suggested location: a thread-local on the JS thread (mirrors `LISTENER_REGISTRY`,
`PENDING_FETCH`):

```rust
// crates/oxibrowser-core/src/js/runtime.rs (near the other thread-locals)
thread_local! {
    /// host_node_id → shadow tree (own node ids + a root ShadowRoot node).
    /// Materialized by attachShadow / shadow-root appendChild. Read by the
    /// DomSnapshot compose pass to flatten slotted content.
    static SHADOW_ROOTS: RefCell<HashMap<u32, ShadowTree>> = const { RefCell::new(HashMap::new()) };
}

struct ShadowTree {
    root_id: u32,                 // the ShadowRoot node
    nodes: HashMap<u32, DomNode>, // shadow subtree (tag == "slot" marks slots)
}
```

### 3.2 Materialize shadow content (replace the JS-only stub)

`attachShadow` (`runtime.rs:9483`) currently returns a JS `DocumentFragment`
that is a plain object — its children never reach any node tree. Upgrade it so
the returned ShadowRoot is a real, node-backed fragment:

- Make `attachShadow` a **native closure** (move it out of the JS bootstrap, the
  way `__oxi_dialog` was moved for dialogs) that:
  1. Mints a `ShadowTree` (new high-id root + node map) keyed by the host's
     `__nodeId`, stored in `SHADOW_ROOTS`.
  2. Returns a JS object exposing `appendChild`/`append`/`innerHTML` setters that
     **write into the shadow tree's node map** (parse the added nodes the way
     `innerHTML` setter already parses via `DomSnapshot::set_inner_html`). A
     minimal first cut: support `shadowRoot.appendChild(document.createElement(...))`
     by mirroring `create_render_element_object` against the shadow map.
- The JS `Element.prototype.attachShadow` then delegates to this native ctor
  (same `__oxi_*` helper pattern used elsewhere).

`shadowRoot` getter / `getRootNode` keep working as today.

### 3.3 Compose pass (the real slot distribution)

Add a **post-pass** after `collect_from_render` in `from_render_document`
(`dom_snapshot.rs:217`, between the `collect_from_render` call at `:233` and
`fill_element_text` at `:244`):

```rust
collect_from_render(root, doc, None, &mut nodes, &mut order, &mut body_id, &mut head_id);
// NEW: flatten shadow trees + distribute slotted content into <slot> positions.
compose_shadow_trees(&mut nodes, &mut order);
fill_element_text(&mut nodes, root as u32);
```

`compose_shadow_trees` is pure local code operating on the `nodes` map. For each
host node id that has an entry in `SHADOW_ROOTS`:

1. **Merge** the shadow subtree's nodes into the snapshot `nodes` map (rewrite
   their ids into the snapshot's id space, or keep the high-id range — just keep
   them disjoint).
2. **Splice** the shadow root's children into the host's `children` in place of
   the host's light children (the host's light children become "assigned" to
   slots).
3. **Distribute slots**: for each `<slot>` in the shadow subtree, replace it
   (in its parent's `children`) with the host's light children whose `slot`
   attribute matches the `<slot name=...>` (default slot = light children with
   no `slot` attr). A `<slot>` with no assigned children falls back to its own
   shadow-DOM default content (the slot's children).
4. Drop the original light children from the host's direct `children` (they've
   been distributed).

The result is the standard **flattened tree** the DOM spec defines — a single
flat `nodes` map that `LayoutEngine`, `build_indices`, `query_selector`, the CDP
DOM domain, `getBoxModel`, etc. all read. No consumer needs changes.

### 3.4 Why this is real (not a stub)

The slot-distribution algorithm is the genuine Shadow DOM composition step
(spec: HTML §"flattening"). It produces correct `assignedSlot` / slot fallback /
default-slot semantics and is reflected in every read path. The single
behavior it cannot affect is `capture_png` (Blitz renders the un-composed
`BaseDocument`) — that's the honestly-scoped Blitz remainder, not a fake.

---

## 4. File layout & anchors

| File | Anchor | Change |
|---|---|---|
| `crates/oxibrowser-core/src/js/runtime.rs` | `:9483` `attachShadow` stub; thread-locals near `:61` | Add `SHADOW_ROOTS` thread-local + native `attachShadow` closure (materialize shadow tree). Move the JS stub to delegate to it. |
| `crates/oxibrowser-core/src/js/dom_snapshot.rs` | `from_render_document` `:217`; `collect_from_render` `:1141`; `DomNode` `:55` | Add `compose_shadow_trees(&mut nodes, &mut order)`; call it between `:241` and `:244`. Optionally add `host`/`slot` fields to `DomNode` if you want richer composition metadata (not strictly required for flattening). |
| `crates/oxibrowser-cdp/src/domains/dom.rs` | geometry methods (`getBoxModel` etc., already shipped) | No change needed — they read the composed snapshot automatically. |
| `crates/oxibrowser-render/src/document.rs` | `capture_png` (Blitz) | **Do not touch** — screenshot slot rendering is the Blitz-gated remainder. |

---

## 5. Suggested commit order

1. `feat(core): SHADOW_ROOTS registry + native attachShadow materializing a shadow subtree`
   (thread-local + native closure; shadow `appendChild` writes to the shadow
   node map). Unit test: `attachShadow` + `shadowRoot.appendChild` populates the
   registry; the shadow node is queryable via a read-back helper.
2. `feat(core): compose_shadow_trees — flatten shadow trees + slot distribution into DomSnapshot`
   (the compose pass in `from_render_document`). Unit test: a custom element
   with a shadow `<slot>` and light children renders the slotted content into
   the snapshot (assert via `LayoutEngine::compute_rect` ordering or a snapshot
   walk).
3. `feat(cdp): probe — shadow DOM slotted content visible to DOM.getBoxModel/getContentQuads`
   (extend `/tmp/oxi-probe` or add an integration test).

Each ships with the workspace gates green: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

---

## 6. Out of scope (explicit)

- **Screenshot rasterization of slotted content** (`capture_png`) — Blitz
  `BaseDocument` has no shadow model and is an external crate. A separate
  initiative (fork/patch Blitz or a shadow-aware renderer).
- JS `assignedSlot` / `assignedNodes()` / `assignedElements()` / `getDestinationInsertionPoints()`
  query APIs on the composed tree (natural follow-up after the registry exists;
  cheap to add to the native `attachShadow` object).
- Declarative shadow DOM (`<template shadowrootmode>` parse-time attach) — needs
  a parser hook; not in scope.
- Closed-mode shadow root hiding from the composed snapshot (trivial once the
  registry tracks `mode`).

---

## 7. Verification checklist for the next session

- [ ] `attachShadow` returns a node-backed ShadowRoot; `shadowRoot.appendChild` writes real nodes into `SHADOW_ROOTS`.
- [ ] A custom element whose shadow root contains `<slot>`, with light-DOM children, shows those children **distributed into the slot position** in:
  - `DomSnapshot` walk (children order),
  - `DOM.getBoxModel` / `getContentQuads` (slotted child has a rect),
  - `DOM.querySelector` (slotted child is findable),
  - `OXI.getMarkdown` / `extract` (slotted text appears).
- [ ] Default slot (no `name`) + named slots both work; a `<slot>` with no assignment shows its fallback content.
- [ ] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test --workspace` all green.

End of remaining-work handoff.
