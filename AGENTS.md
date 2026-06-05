# OxiBrowser AGENTS.md

> This file is loaded into **every** agent session. It is the only context guaranteed to be present
> at the start. Keep it short — detail belongs in `docs/`, not here. Read those files only when the
> task requires them.

## WHAT

OxiBrowser is a **headless browser built in pure Rust**, designed for AI agents and automation.
No Chromium, no V8, no C dependencies — single static binary.

- **Pure Rust** — `boa_engine` (JS), `rustls` (TLS), `html5ever` (HTML). Zero C deps.
- **CDP-compatible** — Puppeteer and Playwright connect without knowing they're talking to OxiBrowser.
- **AI-agent extensions** — `OXI.getMarkdown`, `OXI.getPageInfo` via CDP.
- **Agent-first CLI** — `--json` opt-in, `describe` for schema, `skill` for prompts, `session` for multi-step.

4 crates, ~30K lines of Rust:

| Crate | Role |
|-------|------|
| `oxibrowser` | CLI binary: `fetch`, `extract`, `run`, `session`, `serve`, `describe`, `skill`, `version` |
| `oxibrowser-core` | Engine: Browser→Session→Page→Frame, JS runtime, CSS rendering, network |
| `oxibrowser-cdp` | CDP server: WebSocket + 10 domain handlers |
| `oxibrowser-webapi` | DOM: Document, Node, Tree |

## WHY

Existing headless browsers require Chromium — hundreds of MB, slow startup, massive memory.
OxiBrowser provides the same CDP interface at a fraction of the cost, purpose-built for AI agent
workflows: scraping, automation, screenshot capture, Markdown extraction.

OxiBrowser is a **headless browser built in pure Rust**, designed for AI agents and automation.
No Chromium, no V8, no C dependencies — single static binary.

## HOW

### Build & Run

```bash
cargo build                          # Build everything
cargo test --workspace               # Run all tests
cargo test --workspace -- --ignored   # Include real-website integration tests
cargo run -- fetch <url>             # Fetch and render a URL (markdown default)
cargo run -- serve                   # Start CDP server (default :9222)
cargo run -- session                 # Start interactive JSON REPL
```

### Key Entry Points

| Task | Start here |
|------|-----------|
| Add a CLI subcommand | `crates/oxibrowser/src/main.rs` → add clap variant + handler |
| Add a session command | `crates/oxibrowser/src/session/parser.rs` + `executor.rs` |
| Add a CDP command | `crates/oxibrowser-cdp/src/domains/mod.rs` → add domain file |
| Add a JS Web API | `crates/oxibrowser-core/src/js/runtime.rs` → `create_context()` |
| Add DOM operation | `crates/oxibrowser-core/src/js/dom_snapshot.rs` → `DomSnapshot` + `DomMutation` |
| Add network feature | `crates/oxibrowser-core/src/network/` |
| Add CSS rendering | `crates/oxibrowser-core/src/css/` |

### Architecture at a Glance

Core hierarchy: `Browser` → `Session` → `Page` → `Frame`. Each level owns its children and has a unique atomic ID.

JS (`boa_engine`) runs on a dedicated `std::thread` because `Context` is `!Send`. Communication with
the async main thread goes through `mpsc` channels — one bridge each for fetch, localStorage, and
DOM snapshot sync.

### Thread Safety

All shared state uses `Arc<RwLock>` and `AtomicU64`. No exceptions.

## Detailed Docs (read when the task needs them)


