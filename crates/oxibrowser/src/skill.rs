//! Agent skill guide — printed by `oxibrowser skill`.
//!
//! Designed to be injected into an agent's system prompt or context.
//! ~150 tokens, covers all 3 modes.

/// Return the skill guide as a markdown string.
pub fn skill_text() -> &'static str {
    r#"# OxiBrowser Agent Skills

## 3 Modes

1. **One-shot**: `oxibrowser fetch <url> [flags]` or `oxibrowser extract <url> [flags]`
2. **Automation**: `oxibrowser run <script.yaml>`
3. **Interactive**: `oxibrowser session --json` (stdin commands, stdout JSON)

## Invariant Rules

- ALWAYS add `--json` for machine-readable output (automatic when piped)
- ALWAYS add `--max-bytes 8000` to limit response size
- Use `--summary` first to check if a page is relevant before full read
- Use `--fields url,title,status` to skip large content fields
- Use `session` for multi-step interactions (click → read → click → read)
- Use `run` for complex multi-step automation (YAML scripts)
- NEVER trust eval with untrusted input — use `extract` instead

## One-shot Patterns

```bash
# Read page
oxibrowser fetch <url> --format markdown --json --max-bytes 8000

# Get links
oxibrowser extract <url> --links --json

# Extract elements with attributes
oxibrowser extract <url> --selector "a" --all --attrs text,href --json

# Click then read
oxibrowser fetch <url> --click <selector> --wait <selector> --format markdown --json

# Page metadata only (fast check)
oxibrowser fetch <url> --summary --json

# Run JS
oxibrowser fetch <url> --eval "document.title" --json

# Extract specific text
oxibrowser fetch <url> --extract "h1" --json
```

## Session Workflow

1. Start: `oxibrowser session --json` (run as subprocess)
2. Create tab: `new` → get tab_id
3. Navigate: `goto <tab_id> <url>`
4. Interact: `click/fill/press/eval <tab_id> ...`
5. Extract: `extract/content <tab_id> ... --max-bytes 8000`
6. Close: `close <tab_id>` then `exit`

## Session Commands

`new`, `goto`, `back`, `forward`, `reload`, `click`, `fill`, `press`, `type`, `select`, `check`, `scroll`, `eval`, `extract`, `content`, `screenshot`, `wait`, `close`, `list`, `help`, `exit`

## Output Format

All JSON: `{"ok": true/false, "data": {...}, "error": "...", "error_code": "...", "meta": {"elapsed_ms": N}}`
Check `ok` first. If `false`, read `error_code`.

## Exit Codes

0=success, 1=runtime error, 2=input validation, 3=timeout, 4=network

## Discover Commands

`oxibrowser describe --compact --json` (~200 tokens)
`oxibrowser describe <command> --json` (specific command details)
"#
}
