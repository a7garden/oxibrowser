# Progress

## Status
In Progress

## Tasks
- [x] Round 2 deep review of CDP domain handler changes (9 files)
- [ ] Fix `data-oxi-node-id` attribute injection for `callFunctionOn` node resolution
- [ ] Clean up dead `returnByValue` branch in `Runtime.evaluate`

## Files Changed
- `crates/oxibrowser-cdp/src/domains/runtime.rs` — `callFunctionOn` now evaluates JS via session; `evaluate` reads `returnByValue` (but branches are identical)
- `crates/oxibrowser-cdp/src/domains/dom.rs` — `describeNode` reads real DOM data; `resolveNode` uses deterministic `oxi-node-{id}` objectId; `build_cdp_node` includes attributes
- `crates/oxibrowser-cdp/src/domains/target.rs` — `setDiscoverTargets` emits `targetCreated`; `setAutoAttach` emits `attachedToTarget`
- `crates/oxibrowser-cdp/src/domains/page.rs` — Event ordering fix (Network→Page); reload also emits network events
- `crates/oxibrowser-cdp/src/domains/fetch.rs` — Default pattern fix (no catch-all); `unwrap()` removals
- `crates/oxibrowser-cdp/src/domains/network.rs` — `emit_response_events` helper extracted; `sameSite` hardcoded to "None"
- `crates/oxibrowser-cdp/src/domains/oxi.rs` — Converted from `blocking_read()` to `async` `read().await`
- `crates/oxibrowser-cdp/src/domains/mod.rs` — Added `.await` to OXI and Target dispatch
- `crates/oxibrowser-cdp/src/session.rs` — `expect()` → `ok_or_else()` for event receiver

## Notes
- All 18 workspace tests pass
- `data-oxi-node-id` attribute is never injected into DOM — `callFunctionOn` node resolution always falls back to `document.body`
- Full review written to `/tmp/oxi-review-round2-cdp.md`
