# Contributing to OxiBrowser

Thank you for your interest in contributing to OxiBrowser! This guide covers everything you need to get started.

## Development Setup

### Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust | Stable (latest) | [rustup.rs](https://rustup.rs) |
| Cargo | Latest (via rustup) | Included with Rust |
| Git | Any recent version | System package manager |

### Optional (for servo integration later)

| Tool | Purpose |
|------|---------|
| `cmake` | Building servo native dependencies |
| `clang` | Servo build requirements |

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/oxios/oxibrowser.git
cd oxibrowser

# Build all crates
cargo build

# Run all tests
cargo test --workspace

# Build in release mode
cargo build --release
```

### Verify Your Setup

```bash
# Should compile without errors or warnings
cargo check --workspace

# All tests should pass
cargo test --workspace

# Check formatting
cargo fmt --check

# Run clippy
cargo clippy --workspace -- -D warnings
```

## Project Structure

```
oxibrowser/
├── crates/
│   ├── oxibrowser/          # Binary + CLI
│   ├── oxibrowser-core/     # Core engine (Browser, Session, Page, Frame)
│   ├── oxibrowser-cdp/      # CDP server
│   └── oxibrowser-webapi/   # DOM and WebAPI types
├── docs/                    # Architecture and design docs
├── Cargo.toml               # Workspace definition
├── AGENTS.md                # AI agent convention guide
└── CONTRIBUTING.md          # This file
```

See `AGENTS.md` for detailed module descriptions and `docs/ARCHITECTURE.md` for the full architecture.

## Build Instructions

### Debug Build (Fast Compilation)

```bash
cargo build
```

### Release Build (Optimized)

```bash
cargo build --release
```

### Run the Binary

```bash
# Fetch a page and print HTML
cargo run -- fetch https://example.com

# Start CDP server
cargo run -- serve --host 127.0.0.1 --port 9222
```

### Feature Flags

```bash
# Default build (stub JS runtime, no rendering)
cargo build

# Future: full servo integration
cargo build --features full-servo
```

## Testing

### Run All Tests

```bash
cargo test --workspace
```

### Run Tests for a Specific Crate

```bash
cargo test -p oxibrowser-core
cargo test -p oxibrowser-cdp
cargo test -p oxibrowser-webapi
```

### Run a Specific Test

```bash
cargo test -p oxibrowser-webapi -- test_document_parse
```

### Run Tests with Output

```bash
cargo test -- --nocapture
```

### Writing Tests

Add tests in a `#[cfg(test)] mod tests` block at the bottom of the source file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_my_feature() {
        // Arrange
        let doc = Document::parse("<div id='test'>Hello</div>");

        // Act
        let node_id = doc.query_selector("#test");

        // Assert
        assert!(node_id.is_some());
        let text = doc.text_content(node_id.unwrap());
        assert_eq!(text.as_deref(), Some("Hello"));
    }
}
```

#### Test Categories

| Category | Location | Example |
|----------|----------|---------|
| Unit tests | `#[cfg(test)] mod tests` in source file | DOM parsing, node queries |
| Integration tests | `tests/` directory per crate | Full navigation lifecycle |
| CDP tests | `tests/` in oxibrowser-cdp | Protocol message round-trips |

#### Test Coverage Areas

- **DOM parsing:** All `NodeType` variants, malformed HTML, empty input, very large documents
- **Tree operations:** Parent/child, DFS/BFS traversal, empty tree, single node
- **Cookie jar:** Store, retrieve, domain isolation, empty jar, multiple cookies per domain
- **JS runtime (stub):** All literal types, console.log, globals, unknown expressions
- **CDP dispatch:** Valid domains, unknown domains, valid methods, unknown methods
- **Browser lifecycle:** Create, new_session, close, double-close, max_sessions
- **Session navigation:** Navigate, back, forward, reload, empty history
- **Error conversion:** All `From` implementations

## Code Style

### Formatting

Use `cargo fmt` before every commit:

```bash
cargo fmt --all
```

### Linting

Use `cargo clippy` with strict warnings:

```bash
cargo clippy --workspace -- -D warnings
```

### Naming Conventions

| Item | Convention | Example |
|------|-----------|---------|
| Crates | `oxibrowser-<component>` | `oxibrowser-cdp` |
| Modules | `snake_case` | `network`, `js` |
| Types | `PascalCase` | `BrowserConfig`, `JsEvalResult` |
| Traits | `PascalCase` | (none currently) |
| Functions/methods | `snake_case` | `new_session()`, `fetch_text()` |
| Constants | `SCREAMING_SNAKE_CASE` | `PARSE_ERROR` |
| ID types | `PascalCaseId` | `BrowserId`, `SessionId`, `NodeId` |
| Error enums | `PascalCaseError` | `CoreError`, `CdpError` |

### Documentation

- All public items must have `///` doc comments
- Module-level docs with `//!`
- Include examples in doc comments where applicable

```rust
/// Evaluate a JavaScript expression and return the result.
///
/// In stub mode, handles literals and simple expressions.
/// In servo mode, delegates to the real JS engine.
///
/// # Examples
///
/// ```
/// let mut rt = JsRuntime::new();
/// let result = rt.evaluate("42").await?;
/// assert_eq!(result.value, Some(Value::Number(42.into())));
/// ```
pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
    // ...
}
```

### Error Handling

- Use `thiserror` for library crates (typed errors)
- Use `anyhow` for the binary crate
- Provide `From` conversions for external error types
- Use `Result<T>` aliases defined in each crate

```rust
// Good: typed error with thiserror
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("navigation failed: {0}")]
    NavigationFailed(String),
}

// Good: Result alias
pub type Result<T> = std::result::Result<T, CoreError>;

// Good: From conversion
impl From<url::ParseError> for CoreError {
    fn from(e: url::ParseError) -> Self {
        CoreError::InvalidUrl(e.to_string())
    }
}
```

### Async Code

- Use `tokio` for all async operations
- Use `parking_lot::RwLock` for sync interior mutability (no `.await` while held)
- Use `tokio::sync::RwLock` when the guard must be held across `.await` points
- Use `AtomicBool` / `AtomicU64` for simple shared flags and counters

### CDP Domain Implementation Pattern

Follow the established pattern:

```rust
//! <Domain> domain implementation.

use super::DomainResult;
use crate::protocol::CdpError;
use serde_json::Value;

pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "methodName" => method_name(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown method: <Domain>.{}", method),
        }),
    }
}

fn method_name(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    // Extract parameters, validate, call core, return result
    Ok(Some(serde_json::json!({ "key": "value" })))
}
```

## PR Process

### Before Submitting

1. **Format:** `cargo fmt --all`
2. **Lint:** `cargo clippy --workspace -- -D warnings`
3. **Test:** `cargo test --workspace`
4. **Docs:** Ensure public items have doc comments

### Commit Messages

Follow the conventional commit format:

```
<type>(<scope>): <description>
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `refactor`: Code restructuring without behavior change
- `test`: Adding or updating tests
- `docs`: Documentation changes
- `chore`: Build, CI, tooling changes

**Scopes:**
- `core`: `oxibrowser-core` crate
- `cdp`: `oxibrowser-cdp` crate
- `webapi`: `oxibrowser-webapi` crate
- `cli`: Binary / CLI
- `docs`: Documentation

**Examples:**
```
feat(cdp): implement Page.navigate handler
fix(core): handle URL parse errors in Session::navigate
refactor(webapi): extract query matching into Document method
test(cdp): add dispatch tests for all six domains
docs: add CDP protocol compatibility matrix
chore: update dependencies
```

### Pull Request Template

```markdown
## Description
Brief description of changes.

## Type of Change
- [ ] feat: New feature
- [ ] fix: Bug fix
- [ ] refactor: Code restructuring
- [ ] test: Tests
- [ ] docs: Documentation
- [ ] chore: Tooling/CI

## Testing
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes

## Checklist
- [ ] Public items have doc comments
- [ ] No `#![allow(dead_code)]` in production code
- [ ] Commit messages follow convention
```

### Review Criteria

PRs are reviewed for:
1. **Correctness:** Does it do what it says?
2. **Testing:** Are there sufficient tests?
3. **Documentation:** Are public items documented?
4. **Style:** Does it follow project conventions?
5. **Architecture:** Does it fit the Browser → Session → Page → Frame hierarchy?
6. **Error handling:** Are errors properly typed and propagated?

## Reporting Issues

When reporting bugs, please include:

1. **OxiBrowser version** (`cargo run -- --version` or git commit)
2. **Rust version** (`rustc --version`)
3. **OS and architecture**
4. **Steps to reproduce**
5. **Expected behavior**
6. **Actual behavior**
7. **Logs** (set `RUST_LOG=debug` for verbose output)

## Getting Help

- **Documentation:** Start with `AGENTS.md` for conventions, `docs/ARCHITECTURE.md` for architecture
- **CDP Reference:** `docs/CDP.md` for protocol details
- **Design Decisions:** `docs/DESIGN.md` for rationale

## License

By contributing to OxiBrowser, you agree that your contributions will be licensed under the MIT License.
