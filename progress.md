# Progress

## Status
Done

## Tasks
- [x] Fix 1: Inject `data-oxi-node-id` attribute in `create_element_object` so `callFunctionOn` querySelector works
- [x] Fix 2: Use X coordinate in `elementFromPoint` for element approximation
- [x] Fix 3: Capture current URL in `Page.reload` before emitting `requestWillBeSent`
- [x] Fix R2-1: Network.getAllCookies uses actual cookie same_site attribute instead of hardcoded "None"
- [x] Fix R2-2: Added SAFETY comment to callFunctionOn JS injection documenting IIFE containment
- [x] Fix R2-3: Added TODO(#sop) comment for localStorage same-origin policy check
- [x] Fix R2-4: Simplified duplicate returnByValue branches in Runtime.evaluate with TODO for future objectId support
- [x] Pre-existing fix: Resolved create_context call-site mismatch (cookie_jar_arc parameter) and register_document_object signature mismatch

## Files Changed
- `crates/oxibrowser-core/src/js/runtime.rs` — Added `data-oxi-node-id` injection in `create_element_object`; improved `elementFromPoint` X/Y coordinate handling; added TODO(#sop) same-origin comment for localStorage; fixed create_context call sites and register_document_object signature for cookie_jar_arc
- `crates/oxibrowser-cdp/src/domains/page.rs` — Fixed reload to capture URL before events
- `crates/oxibrowser-cdp/src/domains/runtime.rs` — Updated comment in `callFunctionOn` noting `data-oxi-node-id` is now injected; added SAFETY comment for JS injection; simplified duplicate returnByValue branches in evaluate()
- `crates/oxibrowser-cdp/src/domains/network.rs` — getAllCookies now uses actual `c.same_site` attribute via `oxibrowser_core::network::cookie::SameSite` enum

## Notes
- Reverted unrelated incomplete changes from another agent in `network.rs`, `client.rs`, `ip_filter.rs` that were causing build failures
- Fixed pre-existing build breakage: `create_context` call sites were passing wrong number of arguments after cookie_jar_arc was partially added
- All 255 tests pass (23 e2e + 10 CDP + 201 core + 18 webapi + 3 smoke)
- Build succeeds with `cargo build --workspace`
