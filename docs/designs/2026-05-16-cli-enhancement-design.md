# OxiBrowser CLI Enhancement Design

**Date**: 2026-05-16
**Author**: AI agent
**Status**: Implemented (oxibrowser 0.9.0, oxios-kernel updated)

## Design Principle

**One Browser. One Tool. One Path.**

```
Agent → BrowserTool → oxibrowser_core::Browser (singleton)
                          └── Tab (per-agent, stateful)
                                ├── browse(url)       — one-shot read
                                ├── goto/click/fill   — interactive
                                └── run_script(yaml)  — batch scenarios
```

No subprocess. No CLI from agents. The `oxibrowser` CLI is a developer tool only.

---

## What Was Done

### 1. oxibrowser-core 0.8.0 → 0.9.0

0.9.0 adds `oxibrowser_core::script` module:

```
oxibrowser-core/src/script/
  mod.rs          — pub re-exports
  parser.rs       — YAML → Vec<Step>
  runner.rs       — ScriptRunner executes steps on a Tab
  types.rs        — Step enum, ScriptConfig, ScriptResult, StepResult
```

ScriptRunner handles:
- Variable interpolation (`${var}` → string replacement)
- Error handling (abort/continue, retry with count + delay, auto-screenshot)
- Conditional execution (`if` with JS expression evaluated via `tab.evaluate()`)
- All Tab API actions (goto, click, fill, type, wait, extract, evaluate, etc.)

### 2. BrowserTool: `run_script` Action Added

```rust
"run_script" => {
    let yaml = param_str(&params, "script", ...)?;
    let tab = self.get_or_create_tab().await?;
    let mut runner = oxibrowser_core::script::ScriptRunner::new(&tab);
    let result = runner.run(yaml).await?;
    // result: ScriptResult { steps, vars, success, duration_ms }
}
```

One tool-call = one complex scenario. Tab stays alive between calls.

### 3. `.programs/oxibrowser` Deleted

Agents should never call `oxibrowser` CLI via ExecTool. BrowserTool provides
all browser capabilities in-process. The program registration was a redundant
second path.

### 4. CLI Remains (Developer Tool)

The `oxibrowser` binary exists for human debugging only:
- `oxibrowser run <yaml>` — test scripts (uses ScriptRunner from core)
- `oxibrowser fetch <url>` — quick URL dump
- `oxibrowser browse <url>` — interactive with flags
- `oxibrowser serve` — CDP server for DevTools

Agents never touch it.

---

## BrowserTool — Complete Action List

| Action | Description | State |
|--------|-------------|-------|
| `browse` | URL → Markdown | one-shot |
| `goto` | Navigate | |
| `back` | History back | |
| `forward` | History forward | |
| `reload` | Reload page | |
| `post` | POST request | |
| `click` | Click element | |
| `type` | Type text | |
| `press_key` | Press key combo | |
| `evaluate` | Run JS | |
| `evaluate_await` | Run JS + await Promise | |
| `content` | Page content | |
| `query_all` | Query by selector | |
| `wait_for` | Wait for element | |
| `load_resources` | Load sub-resources | |
| `screenshot` | PNG screenshot | |
| `run_script` | YAML scenario | NEW |
| `close` | Close tab | |

---

## Script DSL Reference

### YAML Schema

```yaml
name: <string>
timeout: <ms>

on_error:
  action: abort | continue   # default: abort
  screenshot: true            # auto-screenshot on error
  retry:
    count: 3
    delay_ms: 500

steps:
  - step_type: goto
    data:
      goto: "https://example.com"
      wait: ".loaded"
  - step_type: fill
    data:
      selector: "#username"
      value: admin
  - step_type: click
    data:
      click: "button[type=submit]"
  - step_type: wait
    data:
      wait: ".dashboard"
      timeout: 10000
  - step_type: evaluate
    data:
      evaluate: "document.querySelector('.user').textContent"
      save: username
  - step_type: extract
    data:
      selector: ".nav-item"
      all: true
      save: nav_items
  - step_type: echo
    data:
      echo: "Logged in as ${username}"
  - step_type: screenshot
    data:
      file: "./dashboard.png"
      width: 1280
```

### Step Types

| Step | Key Fields |
|------|-----------|
| `goto` | `goto: url`, `wait: selector` |
| `back` / `forward` / `reload` | — |
| `post` | `url`, `body`, `content_type` |
| `click` / `dbl-click` / `right-click` / `hover` | `click: selector` |
| `fill` | `selector`, `value` |
| `type` | `selector`, `text` |
| `clear` / `check` / `uncheck` | `selector` |
| `select` | `selector`, `value` |
| `press` | `press: key` |
| `scroll` | `x`, `y` |
| `drag` | `from`, `to` |
| `evaluate` | `evaluate: js`, `await: bool`, `save: var` |
| `wait` | `wait: selector`, `timeout: ms` |
| `extract` | `selector`, `all`, `links`, `text`, `save: var` |
| `content` | `format: markdown\|html\|text\|json`, `save: var` |
| `screenshot` | `file: path`, `width: px` |
| `load_resources` | — |
| `set` | `key: value` (flattened) |
| `echo` | `echo: message` |
| `sleep` | `sleep: ms` |
| `if` | `expression: js`, `then: [steps]`, `else: [steps]` |
| `retry` | `count`, `delay: ms`, `steps: [steps]` |
| `new-tab` | `url` (optional) |
| `close-tab` | — |

### Variables

- `${var}` — string interpolation (done by ScriptRunner, not JS)
- `$$` — literal `$`
- `save: var_name` on evaluate/extract/content steps
- Variables never injected into JS expressions

### if Expression

- Executed via `tab.evaluate(expression)`
- Truthiness: null/false/0/"" → else branch, everything else → then branch
- Same JS context as all evaluate calls

---

## Agent Decision Tree

```
Need to read a URL?              → browse
Need to interact with a page?    → goto → click → fill → ...
Need a complex multi-step flow?  → run_script (one call)
```

---

## Files Changed

| File | Change |
|------|--------|
| `oxios-kernel/Cargo.toml` | `oxibrowser-core` 0.8.0 → 0.9.0 |
| `oxios-kernel/.../browser_tool.rs` | Added `run_script` action |
| `oxios-kernel/.../browser/mod.rs` | Updated module docs |
| `.programs/oxibrowser/` | Deleted (not needed) |
