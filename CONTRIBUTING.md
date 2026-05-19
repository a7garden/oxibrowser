# Contributing to OxiBrowser

Thank you for your interest in contributing to OxiBrowser! This guide covers
everything you need to get started.

## Quick Start

```bash
# 1. Fork and clone
git clone https://github.com/YOUR-USERNAME/oxibrowser.git
cd oxibrowser

# 2. Build
cargo build

# 3. Run tests
cargo test --workspace

# 4. Check code quality
cargo clippy --workspace -- -D warnings
cargo fmt --check

# 5. Create a feature branch
git checkout -b feat/my-feature
```

## Development Requirements

- **Rust** 1.82+ (`rustup update stable`)
- **Git**
- **Internet access** (for integration tests)

No C/C++ compiler, Node.js, or other language toolchains required.

## Project Structure

```
crates/
├── oxibrowser/          # Binary + CLI (entry point)
├── oxibrowser-core/     # Core engine (Browser, Session, Page, Frame, JS)
├── oxibrowser-cdp/      # CDP WebSocket server
└── oxibrowser-webapi/   # DOM tree, CSS selectors
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed architecture docs.

## Code Conventions

### Language

All code, comments, documentation, and commit messages must be in **English**.

### Rust Style

- Edition 2021, MSRV 1.82
- Error handling: `thiserror` for library crates, `anyhow` for binary
- Async: `tokio` throughout
- Sync locks: `parking_lot::RwLock`
- Async locks: `tokio::sync::RwLock`
- Serialization: `serde` + `serde_json`
- IDs: `AtomicU64` / `AtomicU32` with newtype wrappers

### Naming

- Crates: `oxibrowser-<component>` (kebab-case)
- Types/traits: `PascalCase`
- Functions/methods: `snake_case`
- ID types: `PascalCaseId` (`BrowserId`, `SessionId`, etc.)

### Commit Messages

```
<type>(<scope>): <description>

Types: feat, fix, refactor, test, docs, chore
Scopes: core, cdp, webapi, cli, docs
```

Examples:
```
feat(core): add insertBefore DOM method
fix(cdp): handle missing page in Runtime.evaluate
refactor(webapi): extract query_selector to Document
test(core): add mutation persistence test
docs: update architecture diagram
```

## How to Contribute

### Adding a New CDP Domain

1. Create `crates/oxibrowser-cdp/src/domains/<domain>.rs` with a `handle()` function
2. Add `pub mod my_domain` and dispatch case in `domains/mod.rs`
3. Add tests in `#[cfg(test)]` block
4. Update [CDP.md](CDP.md) documentation

### Adding a New JS Web API

1. Register in `create_context()` at `crates/oxibrowser-core/src/js/runtime.rs`
2. For async operations: create an `mpsc` channel bridge
3. For DOM operations: add to `DomSnapshot` and `DomMutation` types
4. Test via `JsRuntime::evaluate()` in a unit test
5. Update README feature table

### Adding a CSS Renderer

1. Add module in `crates/oxibrowser-core/src/css/`
2. Export via `crates/oxibrowser-core/src/css/mod.rs`
3. Wire into `Page` or create new method

## Testing

### Unit Tests

```bash
cargo test --workspace
```

### E2E Tests

```bash
# CDP WebSocket tests
cargo test -p oxibrowser-cdp

# Puppeteer smoke tests (requires Node.js)
cargo test -p oxibrowser --test smoke
```

### Integration Tests (Real Websites)

```bash
# Requires internet access
cargo test --workspace -- --ignored
```

### Writing Tests

- Unit tests: `#[cfg(test)] mod tests` within each file
- Use `tokio::test` for async tests
- Use `Frame::from_doc()` for synchronous DOM testing
- Test edge cases: empty inputs, large inputs, malformed HTML

### Test Requirements

- All new code must have tests
- `cargo test --workspace` must pass
- `cargo clippy --workspace -- -D warnings` must pass
- `cargo fmt --check` must pass

## Pull Request Process

1. **Create a feature branch** from `main`
2. **Write code** following conventions above
3. **Add tests** for new functionality
4. **Run quality checks**:
   ```bash
   cargo test --workspace
   cargo clippy --workspace -- -D warnings
   cargo fmt --check
   ```
5. **Commit** with conventional commit messages
6. **Open a PR** with:
   - Clear description of changes
   - Link to related issues
   - Test output confirming all tests pass

## Reporting Issues

When reporting bugs, please include:

- OxiBrowser version (`oxibrowser version`)
- Rust version (`rustc --version`)
- Operating system
- Steps to reproduce
- Expected vs actual behavior
- Log output (with `--log-level debug`)

## License

By contributing to OxiBrowser, you agree that your contributions will be
licensed under the [GNU Affero General Public License v3](LICENSE).

OxiBrowser is a derivative work of [Lightpanda](https://github.com/lightpanda-io/browser)
(AGPL-3.0, Copyright © 2024 lightpanda contributors). All contributions are made
under the same AGPL-3.0 license.
