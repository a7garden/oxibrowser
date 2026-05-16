# Progress: OXI.getStructuredPage + Markdown WAI-ARIA Heading Support

## Status: ✅ COMPLETE

## Changes Made

### Part A: Markdown WAI-ARIA Heading Support
- **File**: `crates/oxibrowser-webapi/src/dom/document.rs`
- Changed `NodeType::Element { tag, .. }` to `{ tag, attributes }` pattern to access attrs
- Added `role="heading"` check with `aria-level` support before tag-based heading matching
- Default level is 2 when `aria-level` is not specified
- Level is clamped to 1-6 range
- Added 2 unit tests: `test_markdown_aria_heading`, `test_markdown_style_script_skipped`

### Part B: DomSnapshot Structured Data Extraction
- **File**: `crates/oxibrowser-core/src/js/dom_snapshot.rs`
- Added `headings()` — extracts `(level, text)` for `<h1>`-`<h6>` and `role="heading"` elements
- Added `links()` — extracts `(text, href)` for all `<a>` elements
- Added `meta_tags()` — extracts `name/property → content` for `<meta>` elements
- Added helper `deep_text_content()` and `collect_text_recursive()` for recursive text extraction
- Added 5 unit tests covering headings, ARIA headings, links, meta, and empty page

### Part C: OXI.getStructuredPage CDP Command
- **File**: `crates/oxibrowser-cdp/src/domains/oxi.rs`
- Added `getStructuredPage` to the `handle()` match
- Returns JSON with `url`, `title`, `headings[]`, `links[]`, `meta{}`, `linkCount`, `headingCount`
- Supports optional `maxLinks` param (default: 200)

## Test Results
- `cargo test --workspace`: 279 passed, 0 failed
- `cargo clippy --workspace`: 0 warnings

## New Tests Added
1. `test_headings_extraction` — verifies h1/h2/h3 in correct order
2. `test_headings_with_aria_role` — verifies role="heading" + aria-level
3. `test_links_extraction` — verifies links with/without href
4. `test_meta_tags_extraction` — verifies name/property meta extraction
5. `test_structured_data_empty_page` — verifies empty snapshot returns empty
6. `test_markdown_aria_heading` — verifies ARIA headings in markdown output
7. `test_markdown_style_script_skipped` — verifies invisible elements skipped
