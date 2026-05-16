# OxiBrowser CLI Enhancement Design

**Date**: 2026-05-16
**Author**: AI agent
**Status**: Draft

## Problem Statement

Oxios has **two browser paths** that do the same thing:

```
Path A: Agent → BrowserTool → oxibrowser_core::Browser/Tab  (in-process, stateful)
Path B: Agent → ExecTool → "oxibrowser fetch ..." → new Browser  (subprocess, stateless)
```

This is wrong. Two `Browser` instances, two code paths, two maintenance burdens.
For the Agent OS, there should be **one path only**.

---

## The One Path

```
Agent → BrowserTool → oxibrowser_core::Browser (singleton)
                          └── Tab Pool (per-agent or per-task)
                                ├── browse(url)       — one-shot read
                                ├── goto/click/fill   — interactive
                                └── run_script(yaml)  — batch scenarios
```

**Everything** goes through `BrowserTool`. No subprocess. No CLI from agents.

The `oxibrowser` CLI binary becomes a **developer tool** — humans use it to debug and test.
Agents never call it.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                       Oxios Kernel                                  │
│                                                                     │
│  KernelHandle                                                       │
│    └── BrowserApi                                                   │
│          └── Browser (singleton)           ← ONE instance           │
│                └── CookieJar (shared)      ← persistent cookies     │
│                      │                                              │
│                      ├── Tab 1  (Agent A's session)                 │
│                      ├── Tab 2  (Agent B's session)                 │
│                      └── Tab 3  (script execution)                  │
│                                                                     │
│  Agent Tool Registry                                                │
│    └── BrowserTool ──────────────────→ BrowserApi.browser()         │
│          │                                ↑                         │
│          │  action: browse                │                         │
│          │  action: goto / click / fill   │                         │
│          │  action: run_script            │  ← NEW                  │
│          │  action: screenshot / evaluate │                         │
│          └────────────────────────────────┘                         │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│  oxibrowser CLI (developer tool only — NOT in agent tool chain)     │
│                                                                     │
│  Human: oxibrowser fetch <url>                                      │
│  Human: oxibrowser browse <url> --click "#btn" --extract ".result"  │
│  Human: oxibrowser run script.yaml          ← test scripts          │
│  Human: oxibrowser serve                   ← CDP debugging          │
│                                                                     │
│  Never called by agents. No .programs/oxibrowser registration.      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## What Changes

### 1. BrowserTool: Add `run_script` Action

The only missing piece. Everything else already works.

```rust
// In BrowserTool::execute()

"run_script" => {
    let yaml = param_str(&params, "script", "run_script requires 'script'")?;
    let tab = self.get_or_create_tab().await.map_err(|e| e.to_string())?;
    
    // Parse + execute YAML script on the existing Tab
    // (no new Browser, no subprocess)
    let runner = ScriptRunner::new(tab);
    let result = runner.run(yaml).await.map_err(|e| e.to_string())?;
    
    Ok(AgentToolResult::success(serde_json::to_string_pretty(&result).unwrap()))
}
```

Agent usage:
```json
{
  "action": "run_script",
  "script": "name: Login\nsteps:\n  - goto: https://example.com/login\n  - fill: { selector: '#user', value: admin }\n  - click: button[type=submit]\n  - wait: .dashboard\n  - extract: { selector: '.info', all: true }"
}
```

One tool-call. One Tab. One Browser. Result comes back as JSON.

### 2. ScriptRunner in oxibrowser-core

New module. Shared by BrowserTool (agent path) and CLI `run` command (dev path).

```
oxibrowser-core/src/
  script/
    mod.rs          # pub mod + re-exports
    parser.rs       # YAML → Vec<Step>
    runner.rs       # Step execution on Tab
    types.rs        # Step enum, ScriptConfig, ScriptResult
```

```rust
// oxibrowser-core/src/script/runner.rs

pub struct ScriptRunner {
    tab: Tab,
    vars: HashMap<String, Value>,
    on_error: ErrorStrategy,
}

impl ScriptRunner {
    pub fn new(tab: Tab) -> Self { ... }
    
    /// Parse YAML and execute all steps on the Tab.
    pub async fn run(&mut self, yaml: &str) -> Result<ScriptResult> {
        let script = parse_script(yaml)?;
        let mut results = Vec::new();
        for (i, step) in script.steps.iter().enumerate() {
            match self.execute_step(step).await {
                Ok(r) => results.push(r),
                Err(e) => match &self.on_error.action {
                    ErrorAction::Abort => return Err(e.into_step_error(i)),
                    ErrorAction::Continue => results.push(StepResult::error(i, &e)),
                },
            }
        }
        Ok(ScriptResult { steps: results, vars: self.vars.clone() })
    }
}
```

### 3. Remove .programs/oxibrowser

Delete `.programs/oxibrowser/program.toml` and `.programs/oxibrowser/SKILL.md`.

Agents use `BrowserTool` directly (kernel-registered). They should never shell out
to `oxibrowser` via ExecTool — that would create a second Browser instance.

### 4. CLI: Developer Tool Only

The `oxibrowser` binary stays but with a clear scope boundary:

```
oxibrowser SUBCOMMAND

SUBCOMMANDS:
  run <yaml>      Run a script file (uses ScriptRunner from core)
  fetch <url>     One-shot URL fetch (dev debugging)
  browse <url>    Interactive browse with flags (dev debugging)
  serve           Start CDP server (devtools integration)
  version         Print version

REMOVED (not needed — agents use BrowserTool):
  eval            → use BrowserTool action "evaluate"
  extract         → use BrowserTool action "query_all"
  batch           → use BrowserTool action "run_script"
```

The CLI creates its own `Browser` instance (it's a standalone process).
This is fine — it's a dev tool, not part of the agent runtime.

### 5. File Structure

```
oxibrowser-core/src/
  script/              ← NEW
    mod.rs
    parser.rs
    runner.rs
    types.rs

oxibrowser/src/
  main.rs              ← Simplified (dev tool only)
  cmd/
    run.rs             ← oxibrowser run (uses core::ScriptRunner)
    fetch.rs
    browse.rs
    serve.rs

oxios-kernel/src/tools/browser/
  browser_tool.rs      ← Add run_script action

DELETE:
  oxios/.programs/oxibrowser/   ← Agents don't need this program
```

---

## Script DSL

### YAML Schema

```yaml
name: <string>
timeout: <ms>              # default: 30000

on_error:
  action: abort | continue  # default: abort
  screenshot: <bool>        # default: false
  retry:
    count: <n>
    delay_ms: <ms>

steps:
  - <Step>
```

### Step Types

```yaml
# Navigation
- goto: <url>
  wait: <selector>
- back
- forward
- reload
- post:
    url: <url>
    body: <string>
    content_type: <string>

# Interaction
- click: <selector>
- dbl-click: <selector>
- right-click: <selector>
- hover: <selector>
- fill:
    selector: <selector>
    value: <value>
- type:
    selector: <selector>
    text: <string>
- clear: <selector>
- check: <selector>
- uncheck: <selector>
- select:
    selector: <selector>
    value: <value>
- press: <keys>
- scroll:
    x: <px>
    y: <px>
- drag:
    from: <selector>
    to: <selector>

# Content
- evaluate: <expression>
  await: <bool>
  save: <var_name>
- wait: <selector>
  timeout: <ms>
- extract:
    selector: <selector>
    all: <bool>
    links: <bool>
    text: <bool>
  save: <var_name>
- content:
    format: markdown | html | text | json
- screenshot:
    file: <path>
    width: <px>
- load-resources

# Flow Control
- set:
    <name>: <value>
- echo: <message>
- sleep: <ms>
- if:
    expression: <js_expr>    # executed via tab.evaluate()
    then:
      - <steps>
    else:
      - <steps>
- retry:
    count: <n>
    delay: <ms>
    steps:
      - <steps>

# Session
- new-tab:
    url: <url>
- close-tab
```

### Variables

```yaml
- set:
    base_url: "https://example.com"
- goto: "${base_url}/login"           # string interpolation
- evaluate: "document.querySelector('.user').textContent"
  save: username                      # save result to variable
- echo: "Hello ${username}"           # use saved variable
```

Rules:
- `${...}` is pure string replacement, done by ScriptRunner (not JS).
- `$$` escapes to literal `$`.
- Variables never inserted into JS expressions — only into string fields.

### if Expression

```yaml
- if:
    expression: "document.querySelector('.error') !== null"
    then:
      - screenshot:
          file: "./error.png"
      - echo: "Error found"
```

Execution: `tab.evaluate(expression)`, result truthiness determines branch.
Runs in the same JS context as all other evaluate calls.

---

## Error Handling

```yaml
on_error:
  action: abort | continue    # default: abort
  screenshot: true            # auto-screenshot on error
  retry:
    count: 3
    delay_ms: 500
```

Screenshot path on error: `error_step{N}_{timestamp}.png`

JSON error output:
```json
{
  "error": "ElementNotFound",
  "message": "button#submit not found",
  "step": 3,
  "step_name": "click",
  "error_screenshot": "error_step3_20260516_143052.png"
}
```

---

## BrowserTool — Complete Action List

After adding `run_script`, BrowserTool has **one action for every browser need**:

| Action | Description | One-shot? |
|--------|-------------|-----------|
| `browse` | URL → Markdown | ✓ |
| `goto` | Navigate to URL | |
| `back` | History back | |
| `forward` | History forward | |
| `reload` | Reload page | |
| `post` | POST request | |
| `click` | Click element | |
| `type` | Type text into element | |
| `press_key` | Press key combo | |
| `evaluate` | Run JS | |
| `evaluate_await` | Run JS, await Promise | |
| `content` | Get page content | |
| `query_all` | Query elements by selector | |
| `wait_for` | Wait for element | |
| `load_resources` | Load sub-resources | |
| `screenshot` | Capture PNG | |
| `run_script` | Execute YAML scenario | ✓ (NEW) |
| `close` | Close tab | |

**Agent decision tree**:
```
Need to read a URL?           → browse
Need to interact with a page?  → goto → click → fill → ...
Need a complex flow?           → run_script (one call)
```

No subprocess. No CLI. No second Browser instance.

---

## Summary

| Before | After |
|--------|-------|
| Two Browser instances (kernel + CLI subprocess) | One Browser singleton |
| BrowserTool for interactive, CLI for batch | BrowserTool for everything |
| Agents call `oxibrowser` via ExecTool | Agents never call CLI |
| Script logic only in CLI binary | ScriptRunner in `oxibrowser-core` (shared) |
| `.programs/oxibrowser/` registered | Deleted (not needed) |
| 4 usage models (script/REPL/pipeline/split) | 1 usage model (BrowserTool) |

**One Browser. One Tool. One Path.**
