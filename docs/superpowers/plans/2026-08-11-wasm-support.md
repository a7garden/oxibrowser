# WebAssembly Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add WebAssembly 1.0 (MVP) support to OxiBrowser's JS runtime via a `wasmi` ↔ `boa_engine` bridge.

**Architecture:** A new `wasm.rs` module in `oxibrowser-core/src/js/` registers `WebAssembly.*` globals as native boa callables/constructables. All WASM compilation and execution happens synchronously on the JS thread (same as all boa eval). A thread-local context pointer lets WASM host-function callbacks reach the boa `Context`.

**Tech Stack:** `wasmi` v1.x (pure Rust WASM interpreter), `boa_engine` 0.20, existing `oxibrowser-core/src/js/runtime.rs` registration patterns.

**Design spec:** `docs/superpowers/specs/2026-08-11-wasm-support-design.md`

## Global Constraints

- All code is Rust (edition 2024, MSRV 1.96)
- `boa_engine::Context` is `!Send` — WASM ops must be synchronous on the JS thread
- Follow existing registration patterns in `runtime.rs` (`register_global_callable`, JS bootstrap strings, `NativeObjectData`)
- No `unsafe` except the thread-local raw pointer for host-function context access (documented safe-reasoning)
- Binary size increase target: < 5 MB
- Default features on `wasmi` (`std`, `wat`); `simd` disabled

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/oxibrowser-core/src/js/wasm.rs` | **Create** | Entire WASM bridge: engine, registration, value conversion, constructors |
| `crates/oxibrowser-core/src/js/mod.rs` | **Modify** | Add `pub mod wasm;` |
| `crates/oxibrowser-core/src/js/runtime.rs` | **Modify** | Call `wasm::register_wasm_globals(ctx)` in `create_context()` |
| `Cargo.toml` (workspace) | **Modify** | Add `wasmi` to `[workspace.dependencies]` |
| `crates/oxibrowser-core/Cargo.toml` | **Modify** | Add `wasmi = { workspace = true }` |
| `crates/oxibrowser-core/src/js/wasm.rs` (tests) | **Create** | Unit tests inline |

---

### Task 1: Dependency + Scaffold

**Files:**
- Modify: `Cargo.toml` (workspace, line ~53)
- Modify: `crates/oxibrowser-core/Cargo.toml`
- Create: `crates/oxibrowser-core/src/js/wasm.rs`
- Modify: `crates/oxibrowser-core/src/js/mod.rs`

**Interfaces:**
- Produces: `pub struct WasmEngine(wasmi::Engine)`, `pub fn register_wasm_globals(ctx: &mut Context)`

- [ ] **Step 1: Add wasmi to workspace deps**

In `Cargo.toml`, after the `psl = "2"` line in `[workspace.dependencies]`:
```toml
wasmi = "0.40"
```
Check crates.io for latest stable and pin accordingly.

- [ ] **Step 2: Add wasmi to oxibrowser-core deps**

In `crates/oxibrowser-core/Cargo.toml`, add to `[dependencies]`:
```toml
wasmi = { workspace = true }
```

- [ ] **Step 3: Create wasm.rs scaffold**

```rust
//! WebAssembly support — wasmi ↔ boa_engine bridge.
//!
//! Registers `WebAssembly.*` globals so pages using WASM work.
//! All compilation and execution is synchronous on the JS thread.

use boa_engine::{Context, JsResult, JsValue};

/// Opaque wrapper for the wasmi Engine (one per JsRuntime).
pub struct WasmEngine(pub wasmi::Engine);

impl Default for WasmEngine {
    fn default() -> Self {
        Self(wasmi::Engine::default())
    }
}

/// Register all WebAssembly.* globals on the given boa Context.
pub fn register_wasm_globals(_ctx: &mut Context) -> JsResult<()> {
    // TODO: implemented in Task 2+
    Ok(())
}
```

- [ ] **Step 4: Add module declaration**

In `crates/oxibrowser-core/src/js/mod.rs`, add:
```rust
pub mod wasm;
```

- [ ] **Step 5: Wire into create_context**

In `crates/oxibrowser-core/src/js/runtime.rs`, inside `create_context()` (or wherever globals are registered, after the existing `register_global_callable` calls):
```rust
crate::js::wasm::register_wasm_globals(&mut ctx)?;
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build -p oxibrowser-core --features browser 2>/dev/null || cargo build -p oxibrowser-core`
Expected: compiles with no errors (registration is a no-op for now)

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/oxibrowser-core/Cargo.toml crates/oxibrowser-core/src/js/wasm.rs crates/oxibrowser-core/src/js/mod.rs crates/oxibrowser-core/src/js/runtime.rs
git commit -m "feat(wasm): scaffold wasmi dependency + wasm module"
```

---

### Task 2: Value Bridge + WebAssembly.validate + WebAssembly.Module

**Files:**
- Modify: `crates/oxibrowser-core/src/js/wasm.rs`

**Interfaces:**
- Produces: `fn val_to_jsval`, `fn jsval_to_val`, `WasmModuleData`, the `WebAssembly` global object with `validate` and `Module` constructor

- [ ] **Step 1: Write failing test for validate**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, JsValue, Source};

    // Minimal valid WASM module: magic + version 1 + empty (just end marker)
    fn minimal_wasm() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
    }

    #[test]
    fn test_wasm_validate_true() {
        let mut ctx = Context::default();
        register_wasm_globals(&mut ctx).unwrap();
        // Call WebAssembly.validate with valid bytes
        let js = format!(
            r#"WebAssembly.validate(new Uint8Array([{}]))"#,
            minimal_wasm().iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")
        );
        let result = ctx.eval(Source::from_bytes(&js)).unwrap();
        assert_eq!(result, JsValue::Boolean(true));
    }

    #[test]
    fn test_wasm_validate_false() {
        let mut ctx = Context::default();
        register_wasm_globals(&mut ctx).unwrap();
        let result = ctx.eval(Source::from_bytes(
            r#"WebAssembly.validate(new Uint8Array([0,0,0,0]))"#
        )).unwrap();
        assert_eq!(result, JsValue::Boolean(false));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxibrowser-core wasm::tests`
Expected: FAIL — `register_wasm_globals` doesn't register `WebAssembly` yet

- [ ] **Step 3: Implement WebAssembly global + validate**

Implement in `wasm.rs`:
1. `val_to_jsval(val: wasmi::Val, ctx: &mut Context) -> JsResult<JsValue>` — match `Val::I32/I64/F32/F64` → `JsValue::new(...)`
2. `jsval_to_val(v: &JsValue, ty: wasmi::ValType) -> Result<wasmi::Val, String>` — reverse mapping
3. Create a `WebAssembly` namespace `JsObject`, register `validate` as a native function
4. `validate` reads a `Uint8Array` or array-like from args, calls `wasmi::Module::validate(&engine, bytes)`, returns boolean
5. Store the `WasmEngine` as a thread-local or in a `Cell` so `validate` can reach it

Pattern for creating the namespace:
```rust
let wasm_obj = JsObject::default();
let validate_fn = NativeFunction::from_fn_ptr(|_this, args, ctx| {
    // read bytes from args[0], validate, return boolean
    Ok(JsValue::Boolean(is_valid))
});
wasm_obj.set("validate", JsValue::from(validate_fn), false, ctx)?;
// ... more members
ctx.register_global_callable(
    JsString::from("WebAssembly").into(),
    0,
    NativeFunction::from_fn_ptr(|_, _, ctx| {
        // return the WebAssembly namespace object
    }),
)?;
```

Or simpler: register `WebAssembly` as a global property that is the namespace object.

- [ ] **Step 4: Implement Module constructor**

`WebAssembly.Module(bytes)`:
1. Read `Uint8Array` bytes from args
2. `wasmi::Module::new(&engine, &bytes)` — compile
3. On error, throw `CompileError`
4. Store the `Module` as `NativeObjectData` on the returned `JsObject`
5. The returned object has internal `__wasm_module` data

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p oxibrowser-core wasm::tests`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/oxibrowser-core/src/js/wasm.rs
git commit -m "feat(wasm): WebAssembly.validate + Module constructor + value bridge"
```

---

### Task 3: Instance + Exported Function Calls

**Files:**
- Modify: `crates/oxibrowser-core/src/js/wasm.rs`

**Interfaces:**
- Produces: `WasmInstanceData`, `WebAssembly.Instance` constructor, `instance.exports` property, exported-function callable wrappers

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_wasm_instantiate_and_call() {
    let mut ctx = Context::default();
    register_wasm_globals(&mut ctx).unwrap();
    // WAT: (module (func (export "add") (param i32 i32) (result i32)
    //   local.get 0 local.get 1 i32.add))
    let js = r#"
        var wasmBytes = new Uint8Array([
            0x00,0x61,0x73,0x6d, 0x01,0x00,0x00,0x00,
            // type section: (func (param i32 i32) (result i32))
            0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f,
            // function section
            0x03,0x02,0x01,0x00,
            // export section: "add" -> func 0
            0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00,
            // code section
            0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b
        ]);
        var module = new WebAssembly.Module(wasmBytes);
        var instance = new WebAssembly.Instance(module);
        instance.exports.add(2, 3)
    "#;
    let result = ctx.eval(Source::from_bytes(js)).unwrap();
    assert_eq!(result, JsValue::Integer(5));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxibrowser-core wasm::tests::test_wasm_instantiate_and_call`
Expected: FAIL — `WebAssembly.Instance` not implemented

- [ ] **Step 3: Implement Instance constructor**

1. Read `Module` from args[0] (extract from NativeObjectData)
2. Read optional import object from args[1] (Task 4 handles imports; for now, pass empty linker)
3. `wasmi::Store::new(&engine, 0u32)` — create store with u32 host state
4. `Linker::new(&engine)` — empty linker (no imports yet)
5. `linker.instantiate(&mut store, &module)` — instantiate
6. Build `exports` JsObject:
   - Iterate `instance.exports()`, for each `Extern::Func` create a callable `JsObject` wrapping the `Func` + store reference
   - For `Extern::Memory`, create Memory wrapper (Task 4)
   - For `Extern::Table`, create Table wrapper (Task 5)
7. Store `WasmInstanceData { instance, store }` as NativeObjectData

For exported function calls, the callable JsObject must hold:
- The `Func` reference
- A reference to the store (mutable access needed for `func.call(&mut store, args)`)
- The function signature (param/result types) for value conversion

**Key challenge:** The store must be mutable when calling an exported function, but boa doesn't give us mutable access to instance data during a `[[Call]]`. Solution: store the `Store` in a `Rc<RefCell<Store<WasmHostState>>>` inside the NativeObjectData. Host functions and export calls borrow it mutably.

- [ ] **Step 4: Implement exported function callable**

```rust
// The export-call closure receives the JsObject's internal data.
// It reads params from JsValue args, converts to wasmi Val,
// calls func.call(&mut *store.borrow_mut(), &vals),
// converts results back to JsValue.
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p oxibrowser-core wasm::tests`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/oxibrowser-core/src/js/wasm.rs
git commit -m "feat(wasm): Instance constructor + exported function calls"
```

---

### Task 4: Import Resolution (Host Functions)

**Files:**
- Modify: `crates/oxibrowser-core/src/js/wasm.rs`

**Interfaces:**
- Produces: import-object parsing, host-function registration via wasmi `Linker`, thread-local context pointer for callbacks

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_wasm_host_function_import() {
    let mut ctx = Context::default();
    register_wasm_globals(&mut ctx).unwrap();
    // WAT: (module
    //   (import "env" "double" (func $double (param i32) (result i32)))
    //   (func (export "call_double") (result i32)
    //     (call $double (i32.const 21)))
    // )
    let js = r#"
        var wasmBytes = new Uint8Array([
            0x00,0x61,0x73,0x6d, 0x01,0x00,0x00,0x00,
            // type: (func (param i32) (result i32))
            0x01,0x06,0x01,0x60,0x01,0x7f,0x01,0x7f,
            // import: "env"."double" type 0
            0x02,0x0d,0x01,0x03,0x65,0x6e,0x76,0x06,0x64,0x6f,0x75,0x62,0x6c,0x65,0x00,0x00,
            // function: func 0 (type 0)
            0x03,0x02,0x01,0x00,
            // export: "call_double" -> func 1
            0x07,0x0e,0x01,0x0b,0x63,0x61,0x6c,0x6c,0x5f,0x64,0x6f,0x75,0x62,0x6c,0x65,0x00,0x01,
            // code: func 1: call $double(21)
            0x0a,0x09,0x01,0x07,0x00,0x41,0x15,0x10,0x00,0x0b
        ]);
        var module = new WebAssembly.Module(wasmBytes);
        var instance = new WebAssembly.Instance(module, {
            env: { double: function(x) { return x * 2; } }
        });
        instance.exports.call_double()
    "#;
    let result = ctx.eval(Source::from_bytes(js)).unwrap();
    assert_eq!(result, JsValue::Integer(42));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxibrowser-core wasm::tests::test_wasm_host_function_import`
Expected: FAIL — import object not parsed

- [ ] **Step 3: Implement import resolution**

1. When `Instance(module, importObject)` receives args[1], iterate the import object:
   - For each module name (property key) → namespace object
   - For each function name in namespace → JsValue (must be callable)
2. For each expected import from `module.imports()`, look it up in the JS import object
3. For `FuncType` imports, use `Linker::define` + a host function wrapper:
   - The host function wrapper stores the JsValue callable in a registry keyed by `(module_name, func_name)`
   - When called, it retrieves the JsValue, converts wasmi params to JsValue args, calls the JS function via the boa Context (thread-local), converts the return JsValue back to wasmi Val
4. Thread-local context pointer:
   ```rust
   thread_local! {
       static WASM_HOST_CTX: Cell<*mut Context<'static>> = Cell::new(ptr::null_mut());
   }
   ```
   Set before calling exported functions; read in host function wrapper; clear after.

**Critical:** The host function closure passed to wasmi must be `Fn` (called multiple times). It reads the JS callable from a thread-local registry (not from captured state, since the closure signature is constrained by wasmi's `IntoFunc` trait).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p oxibrowser-core wasm::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/oxibrowser-core/src/js/wasm.rs
git commit -m "feat(wasm): host function import resolution"
```

---

### Task 5: WebAssembly.Memory

**Files:**
- Modify: `crates/oxibrowser-core/src/js/wasm.rs`

**Interfaces:**
- Produces: `WasmMemoryData`, `Memory` constructor, `.buffer`, `.grow()`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_wasm_memory_grow_and_read() {
    let mut ctx = Context::default();
    register_wasm_globals(&mut ctx).unwrap();
    let js = r#"
        var memory = new WebAssembly.Memory({ initial: 1 });
        memory.grow(1);
        memory.buffer.byteLength
    "#;
    let result = ctx.eval(Source::from_bytes(js)).unwrap();
    // 2 pages * 65536 = 131072
    assert_eq!(result, JsValue::Integer(131072));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxibrowser-core wasm::tests::test_wasm_memory_grow_and_read`
Expected: FAIL

- [ ] **Step 3: Implement Memory**

1. `Memory({initial, maximum})` constructor:
   - Read `initial` (required) and `maximum` (optional) from args
   - `wasmi::Memory::new(&mut store, MemoryType::new(initial, maximum, false))`
   - Store as NativeObjectData
2. `.buffer` getter:
   - Returns a JsObject with `.byteLength` property and indexed access
   - `memory.data(&store).len()` → `byteLength`
   - Indexed `get(i)` → `JsValue::Integer(memory.data(&store)[i])`
   - Indexed `set(i, v)` → `memory.data_mut(&mut store)[i] = v`
3. `.grow(delta)`:
   - `memory.grow(&mut store, delta)` → returns old page count
   - Returns the old size (number of pages before grow)

**Store access:** Memory operations need `&mut Store`. The `Store` lives inside the `WasmInstanceData` (or standalone for `new WebAssembly.Memory()`). Use `Rc<RefCell<Store<...>>>`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p oxibrowser-core wasm::tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/oxibrowser-core/src/js/wasm.rs
git commit -m "feat(wasm): WebAssembly.Memory constructor + buffer + grow"
```

---

### Task 6: Table + Error Types + compile/instantiate Promises

**Files:**
- Modify: `crates/oxibrowser-core/src/js/wasm.rs`

**Interfaces:**
- Produces: `Table` constructor + methods, error constructors, `WebAssembly.compile` / `instantiate` async wrappers

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn test_wasm_table() {
    let mut ctx = Context::default();
    register_wasm_globals(&mut ctx).unwrap();
    let js = r#"
        var table = new WebAssembly.Table({ element: "anyfunc", initial: 2 });
        table.length
    "#;
    let result = ctx.eval(Source::from_bytes(js)).unwrap();
    assert_eq!(result, JsValue::Integer(2));
}

#[test]
fn test_wasm_compile_error() {
    let mut ctx = Context::default();
    register_wasm_globals(&mut ctx).unwrap();
    let js = r#"
        try {
            new WebAssembly.Module(new Uint8Array([0xFF, 0xFF]));
            "no error";
        } catch (e) {
            e instanceof WebAssembly.CompileError ? "compile error" : e.message;
        }
    "#;
    let result = ctx.eval(Source::from_bytes(js)).unwrap();
    assert_eq!(result, JsValue::String("compile error".into()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p oxibrowser-core wasm::tests::test_wasm_table wasm::tests::test_wasm_compile_error`
Expected: FAIL

- [ ] **Step 3: Implement Table**

1. `Table({element, initial, maximum})` constructor
2. `.length` getter → current table size
3. `.get(index)` → returns the element (funcref or null)
4. `.set(index, value)` → sets the element
5. `.grow(delta, value)` → grows the table, returns old length

- [ ] **Step 4: Implement Error constructors**

Register three error constructors as subclasses of `Error`:
- `WebAssembly.CompileError` — thrown on invalid module bytes
- `WebAssembly.LinkError` — thrown on missing imports or type mismatches
- `WebAssembly.RuntimeError` — thrown on WASM traps (unreachable, OOB, etc.)

Pattern: create each as a `JsObject` with `name` property set, prototype chain to `Error.prototype`.

- [ ] **Step 5: Implement compile/instantiate Promises**

`WebAssembly.compile(bytes)` → `Promise.resolve(new Module(bytes))`
`WebAssembly.instantiate(bytes, imports)` → `Promise.resolve(new Instance(new Module(bytes), imports))`
`WebAssembly.instantiate(module, imports)` → `Promise.resolve(new Instance(module, imports))`

Since boa 0.20 has `Promise::resolve`, use it to wrap the synchronous result.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p oxibrowser-core wasm::tests`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/oxibrowser-core/src/js/wasm.rs
git commit -m "feat(wasm): Table + error types + compile/instantiate promises"
```

---

### Task 7: Full-Workspace Verification

**Files:** None (verification only)

- [ ] **Step 1: Build entire workspace**

Run: `cargo build --release --features browser`
Expected: compiles, binary size increases by < 5 MB

- [ ] **Step 2: Run all tests**

Run: `cargo test --workspace`
Expected: all existing tests pass + new wasm tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Manual CDP smoke test**

```bash
cargo run -- serve &
sleep 1
# Connect via CDP, evaluate a WASM snippet
# Use the acceptance harness pattern
```

- [ ] **Step 5: Verify binary size**

```bash
ls -lh target/release/oxibrowser
# Should be < 49 MB (44 MB current + < 5 MB wasmi)
```

- [ ] **Step 6: Commit + CHANGELOG**

Update `CHANGELOG.md` with a new `## [Unreleased] ### Added` entry:
```markdown
- **WebAssembly support** — `WebAssembly.compile`/`instantiate`/`validate`/`Module`/`Instance`/`Memory`/`Table` now work via a `wasmi` (pure Rust WASM interpreter) ↔ `boa_engine` bridge. Pages using WASM modules (compile, instantiate, call exports, host-function imports, linear memory) are supported. WebAssembly 1.0 (MVP) scope.
```

```bash
git add CHANGELOG.md
git commit -m "feat(wasm): WebAssembly 1.0 MVP support via wasmi"
```

---

## Self-Review Notes

**Spec coverage:** All items from the design spec are covered — Engine, Module, Instance, Memory, Table, validate, compile, instantiate, errors, value bridge, host functions, store management.

**Type consistency:** `WasmEngine` holds `wasmi::Engine`. All `Module`/`Instance`/`Memory`/`Table` wrappers use `Rc<RefCell<Store<WasmHostState>>>` for shared mutable store access. `WasmHostState = u32` (placeholder).

**Key risk:** The `Rc<RefCell<Store>>` pattern means `Store` borrows are runtime-checked. A host function that recursively calls back into JS that then calls another WASM export would panic on double-borrow. Mitigation: use `try_borrow_mut` and queue/defer recursive calls. Document this limitation.
