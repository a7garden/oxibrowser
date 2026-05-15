# Progress

## Status
Done

## Tasks
- [x] Fix 1: Inject `data-oxi-node-id` attribute in `create_element_object` so `callFunctionOn` querySelector works
- [x] Fix 2: Use X coordinate in `elementFromPoint` for element approximation
- [x] Fix 3: Capture current URL in `Page.reload` before emitting `requestWillBeSent`

## Files Changed
- `crates/oxibrowser-core/src/js/runtime.rs` — Added `data-oxi-node-id` injection in `create_element_object`; improved `elementFromPoint` X/Y coordinate handling
- `crates/oxibrowser-cdp/src/domains/page.rs` — Fixed reload to capture URL before events
- `crates/oxibrowser-cdp/src/domains/runtime.rs` — Updated comment in `callFunctionOn` noting `data-oxi-node-id` is now injected

## Notes
- Reverted unrelated incomplete changes from another agent in `network.rs`, `client.rs`, `ip_filter.rs` that were causing build failures
- All 201 core tests + 18 webapi tests pass
- Build succeeds with `cargo build --workspace`
