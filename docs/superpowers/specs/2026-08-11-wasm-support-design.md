# WebAssembly Support — Design Spec

> **Status:** Design complete, pending implementation.
> **Author:** 2026-08-11 session
> **Scope:** `oxibrowser-core` JS runtime (`boa_engine` ↔ `wasmi` bridge)

## 1. Goal

Add WebAssembly 1.0 (MVP) support to OxiBrowser's JS runtime so that pages
using `WebAssembly.compile` / `instantiate` / `Memory` / `Table` work correctly.
This closes the "WASM: no" gap identified in the headless-browser comparison.

## 2. Architecture Decision: wasmi

**Chosen:** [`wasmi`](https://crates.io/crates/wasmi) v1.x — pure Rust WASM interpreter.

| Criterion | wasmi | wasmtime | wasm3 |
|-----------|-------|----------|-------|
| Language | Pure Rust | Rust + Cranelift | C |
| Binary add | ~2–3 MB | ~15–20 MB | ~1 MB |
| Execution | Interpreter | Cranelift JIT | Interpreter |
| Spec compliance | Full (spectest) | Full | Partial |
| `!Send` friendly | Yes | Complex | N/A (C FFI) |
| License | Apache-2.0 | Apache-2.0 | MIT |

**Rationale:** wasmi matches OxiBrowser's "Rust-first, tiny footprint" philosophy.
wasmtime's Cranelift JIT adds ~20 MB to the binary (we are at 44 MB; that would
push us to ~64 MB) and is overkill for AI-agent scraping/automation workloads.
wasm3 requires C FFI, violating the pure-Rust constraint.

## 3. Integration Constraints

### 3.1 Threading Model

`boa_engine::Context` is `!Send` — all JS executes on a dedicated `std::thread`
(the existing `JsRuntime` pattern). wasmi's `Engine`, `Module`, and `Store` are
also not `Send`-safe for cross-thread use. **Therefore, WASM compilation and
execution happen on the JS thread**, synchronously, inside boa's `eval` calls.

This is architecturally clean: when JS calls `WebAssembly.instantiate(bytes)`,
the boa native function executes on the JS thread, wasmi compiles/instantiates
synchronously, and the result is returned to JS as a `JsValue`.

### 3.2 Host Function Callbacks

WASM modules can import host functions (e.g., `import "env" "log" (func ...)`).
These host functions need to call back into JS (the import object is a JS object
with function values). The wasmi `Linker::func_wrap` closure receives a
`Caller<'_, T>` — but it cannot directly hold a `&mut Context` (lifetime conflict
with the closure's `'static` requirement).

**Solution:** Thread-local context pointer, following the existing OxiBrowser
pattern (`LISTENER_REGISTRY`, `ACTIVE_CONTEXT_ID`, etc.):

```rust
thread_local! {
    /// Set during WASM host-function execution so host fns can reach the boa Context.
    /// Safe: only the JS thread accesses it; set before call, cleared after.
    static WASM_HOST_CTX: Cell<*mut boa_engine::Context<'static>> = Cell::new(ptr::null_mut());
}
```

Before calling a WASM exported function (which may call back into host fns), set
the pointer. Host functions read it to access the boa `Context` for JS calls.
Clear after the call returns or traps.

### 3.3 Value Bridge

wasmi `Val` ↔ boa `JsValue`:

| WASM type | wasmi `Val` | boa `JsValue` |
|-----------|-------------|---------------|
| `i32` | `Val::I32(i32)` | `JsValue::Number(f64)` |
| `i64` | `Val::I64(i64)` | `JsValue::BigInt` (or `Number` if safe) |
| `f32` | `Val::F32(f32)` | `JsValue::Number(f64)` |
| `f64` | `Val::F64(f64)` | `JsValue::Number(f64)` |
| `externref` | `Val::ExternRef` | `JsValue` (opaque, stored in a registry) |
| `funcref` | `Val::Func(Func)` | wrapped callable `JsObject` |

### 3.4 Memory

`WebAssembly.Memory` exposes a `.buffer` property that is an `ArrayBuffer`.
wasmi `Memory` provides `data(&store) -> &mut [u8]` and `data_mut()`. The bridge
wraps this as a boa object with:
- `.buffer` → a `Uint8Array`-backed view (or a custom ArrayBuffer-like object)
- `.grow(delta)` → grows memory, returns old size
- `.valueOf()` / inspected as `[object WebAssembly.Memory]`

**Implementation note:** boa 0.20 does not have a native `ArrayBuffer`. The
existing Canvas 2D shim uses a plain `JsObject` with indexed properties. We
follow the same pattern: `.buffer` is a `JsObject` with `.byteLength` and
indexed byte access via `get`/`set` traps. This is sufficient for agent
automation; true `ArrayBuffer` transfer semantics can come later.

## 4. API Surface

All registered as native constructables/callables via `register_wasm_globals()`:

### 4.1 `WebAssembly` namespace

| Member | Type | Behavior |
|--------|------|----------|
| `compile(BufferSource)` | Function → `Promise<Module>` | Compiles bytes, returns a resolved Promise (sync under the hood) |
| `instantiate(bytes|Module, importObject)` | Function → `Promise<Instance \| {module, instance}>` | Compiles + instantiates, returns Promise |
| `validate(BufferSource)` | Function → `boolean` | Validates bytes are a valid WASM module |
| `Module(bytes)` | Constructor | Compiles and stores wasmi `Module` internally |
| `Module.exports(module)` / `.imports(module)` | Static | Returns export/import descriptors |
| `Instance(module, importObject)` | Constructor | Instantiates, exposes `.exports` |
| `Memory({initial, maximum})` | Constructor | Creates linear memory |
| `Table({element, initial, maximum})` | Constructor | Creates function reference table |
| `CompileError` / `LinkError` / `RuntimeError` | Constructors | Error subtypes extending `Error` |

### 4.2 Instance exports

`instance.exports` is a plain `JsObject` whose properties map to WASM exports:
- Exported functions → boa `JsObject` with a `[[Call]]` internal that calls the
  wasmi `TypedFunc` (or untyped `Func::call`)
- Exported memories → `WebAssembly.Memory` wrapper
- Exported tables → `WebAssembly.Table` wrapper
- Exported globals → accessor properties (getter only for `immutable`)

## 5. File Structure

```
crates/oxibrowser-core/src/js/
├── wasm.rs          ← NEW (~700-900 lines): the entire WASM bridge
├── runtime.rs       ← MODIFY: call register_wasm_globals() in create_context()
├── mod.rs           ← MODIFY: pub mod wasm;
```

### `wasm.rs` internal structure

```rust
// ── wasmi engine holder ──────────────────────────────────────
pub struct WasmEngine(Engine);   // constructed once per JsRuntime

// ── boa-internal opaque data ─────────────────────────────────
// Module/Instance/Memory/Table wrappers store wasmi handles as
// boa NativeObjectData (Box<dyn Any>):
struct WasmModuleData(Module);
struct WasmInstanceData { instance: Instance, store: Store<WasmHostState> }
struct WasmMemoryData(Memory);
struct WasmTableData(Table);

// ── host state for wasmi Store ───────────────────────────────
type WasmHostState = u32; // placeholder; real context via thread-local

// ── registration entry point ─────────────────────────────────
pub fn register_wasm_globals(ctx: &mut Context) { ... }

// ── value conversion ─────────────────────────────────────────
fn val_to_jsval(val: Val, ctx: &mut Context) -> JsResult<JsValue>;
fn jsval_to_val(v: &JsValue, ty: ValueType) -> Result<Val, JsValue>;

// ── per-constructor native functions ─────────────────────────
fn wasm_compile(...) -> JsResult<JsValue>;   // returns Promise<Module>
fn wasm_instantiate(...) -> JsResult<JsValue>;
fn wasm_validate(...) -> JsResult<JsValue>;
fn module_constructor(...) -> JsResult<JsValue>;
fn instance_constructor(...) -> JsResult<JsValue>;
fn memory_constructor(...) -> JsResult<JsValue>;
fn table_constructor(...) -> JsResult<JsValue>;
```

## 6. Cargo.toml changes

**`Cargo.toml` (workspace):**
```toml
[workspace.dependencies]
wasmi = "0.40"   # or latest 1.x; verify at impl time
```

**`crates/oxibrowser-core/Cargo.toml`:**
```toml
wasmi = { workspace = true }
```

Default features (`std`, `wat`) are fine. `simd` is disabled (not needed).

## 7. Scope Boundaries

### In scope (WebAssembly 1.0 MVP + common post-MVP)
- ✅ i32/i64/f32/f64 value types
- ✅ Imported/exported functions, memories, tables, globals
- ✅ `externref` reference type (if wasmi supports in v1.x — verify)
- ✅ Import resolution from JS import object
- ✅ `Memory.grow` + `buffer` access
- ✅ `Table.get/set/grow/length`
- ✅ Error types (`CompileError`, `LinkError`, `RuntimeError`)
- ✅ `Module.exports()` / `Module.imports()` static descriptors

### Out of scope (future work)
- ❌ SIMD (wasmi `simd` feature; not needed for agent automation)
- ❌ Shared memory / threads (`shared` flag on Memory)
- ❌ Exception handling proposal
- ❌ GC proposal (`anyref` beyond basic `externref`)
- ❌ Native `ArrayBuffer` / `Uint8Array` (uses boa object wrapper instead)
- ❌ WASI (system interface — not a browser concern)
- ❌ Streaming compilation (`WebAssembly.compileStreaming` — requires fetch+Response integration; can be added later)

## 8. Testing Strategy

### Unit tests (in `wasm.rs`)
1. **Compile + call** — instantiate `(module (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))`, call `add(2, 3)`, assert `5`.
2. **Host function import** — module imports `"env"."log"`, JS provides a function, WASM calls it with `i32.const 42`, assert JS function received `42`.
3. **Memory write/read** — WASM writes `42` to memory offset 0, JS reads `memory.buffer[0]`, assert `42`.
4. **Validate** — `WebAssembly.validate(new Uint8Array([0,0x61,0x73,0x6d,...]))` returns `true`; garbage bytes return `false`.
5. **Error paths** — invalid bytes throw `CompileError`; missing import throws `LinkError`; `unreachable` instruction throws `RuntimeError`.

### Integration test (CDP)
- `Runtime.evaluate` with a JS snippet that compiles, instantiates, and calls a WASM module, returning the result.

### Acceptance harness
- `acceptance/wasm/` — a minimal HTML page with inline JS that loads a tiny WASM module, calls it, and writes the result to the DOM. The harness navigates, waits for the result selector, and verifies.

## 9. Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| wasmi `Store` not `Send` → can't use in async | Low | All WASM ops are sync on JS thread, like existing boa calls |
| boa 0.20 lacks `ArrayBuffer` → memory access is lossy | Medium | Use object wrapper; document limitation; upgrade when boa adds `ArrayBuffer` |
| Binary size increase >5 MB | Low | wasmi is ~2-3 MB; verify after integration |
| `i64` → JS `BigInt` interop in boa 0.20 | Medium | Test boa `BigInt` support; fallback to `Number` if `i64` fits in `Number.MAX_SAFE_INTEGER` |
| WASM traps crash JS thread | Low | wasmi returns `Error` on trap; catch and convert to `RuntimeError` JsValue |

## 10. Open Questions for Implementation

1. **wasmi exact version** — latest stable on crates.io at impl time (v0.40+ or v1.x). API is stable across these.
2. **boa `BigInt` support** — verify boa 0.20's `BigInt` works for `i64` WASM values. If not, use `Number` (safe up to 2^53).
3. **`externref` in wasmi v1.x** — verify the reference-types proposal is supported. If not, defer externref to a later phase.
4. **Fuel/gas metering** — wasmi supports fuel-based execution limits. Consider adding a default fuel limit to prevent infinite WASM loops from hanging the JS thread. Decision: add configurable fuel (default: 10M instructions, overridable).
