---
name: dsl-design-decisions
description: Use when reviewing or seeing proposals to swap EdgionDSL for Rhai / mlua / rune / Boa, or to add user-defined functions, closures, dynamic code loading, or any other ergonomic feature already considered and rejected during the `dsl-vm-rhai-research` series. The decision is locked; this is the canonical home for the engine-selection rationale.
---

# EdgionDSL custom VM vs Rhai / mlua / rune / Boa — DESIGN-DECISION (LOCKED)

**Date investigated**: 2026-05-13 → 2026-05-15
**Status**: Locked. EdgionDSL keeps its custom bytecode VM. Re-open only on the triggers listed below.
**Originating analysis**: `tasks/todo/dsl-vm-rhai-research/` (7-report investigation series, sub-reports deleted 2026-05-19 after closure; per-finding memos under `04-review/` are the canonical successors).

## TL;DR

Four off-the-shelf scripting engines (Rhai, mlua, rune, Boa) were evaluated end-to-end against EdgionDSL's actual constraints: Controller compiles → Gateway executes (cross-process bytecode), sandbox enforced inside the engine, no user-defined functions, no global mutable state. **No off-the-shelf engine clears all four constraints** — every candidate either lacks serializable bytecode, lacks a built-in sandbox, exposes a closure / module / dynamic-code feature that becomes attack surface, or introduces a thread-local / global interner. The custom VM is ~7k lines of safe Rust (incl. inline tests) that meets all four; replacing it would be a rewrite at equal or greater complexity to gain features that are explicitly out of scope.

**Decision**: keep the custom bytecode VM. Specific deferred / declined alternatives are catalogued below with reverse-links.

## When to apply this memo

Stop and read this memo before re-investigating proposals of the form:

- "Why not just use Rhai / mlua / rune / Boa instead of maintaining a custom VM"
- "Add user-defined functions / closures to EdgionDSL"
- "Add an `import` / `require` / `module` system so scripts can be split across files"
- "Add a REPL or interactive shell for EdgionDSL"
- "Use Rhai's `Dynamic` / `OptimizationLevel` / `Package` system instead of the explicit `BuiltinId` enum"
- "Switch from in-house bytecode to wasm bytes / Lua bytecode / Rhai serde-bytecode"
- "Add a global string interner / shared-mutable `ctx`-like value across scripts"

If the proposal does not match a listed re-evaluation trigger at the bottom, the decision below stands.

## Decision summary table

| Engine | License | Cross-process serializable bytecode | Built-in sandbox | User-defined fn / closures | Code body (LoC) | Verdict |
|--------|---------|--------------------------------------|------------------|---------------------------|-----------------|---------|
| EdgionDSL (custom) | Apache-2.0 (project) | ✅ JSON+base64 bytecode, version-tagged | ✅ `Validator` + `VmLimits` in-engine | ❌ deliberately none | ~7k safe Rust | **kept** |
| Rhai 1.x | MIT/Apache-2.0 | ⚠️ `AST` is `Serialize`-able but unstable; not designed for cross-process distribution | ⚠️ via host wrappers; no in-engine validator | ✅ closures + modules + packages | ~25k Rust | **rejected** (constraints 1 + 3) |
| mlua (LuaJIT bind) | MIT | ❌ Lua bytecode is intra-process; Lua 5.4 disabled `string.dump` for restricted envs | ❌ requires manual hostile-environment hardening (debug lib, `os.execute`, ...) | ✅ full Lua + metatables | C dep + bindings | **rejected** (constraints 1 + 2 + 3 + 4) |
| rune | MIT/Apache-2.0 | ⚠️ unit `Unit` is serializable but cross-version compat not promised | ⚠️ runtime budget + module gating; no validator | ✅ first-class fn / closures | ~30k Rust | **rejected** (constraints 3 + project-maturity) |
| Boa (JS) | MIT/Unlicense | ❌ no serializable bytecode (interpreter on AST) | ❌ inherits JS-world complexity (prototype chain, `eval`, ...) | ✅ full ECMAScript | ~50k Rust + spec | **rejected** (constraints 1 + 2 + 3) |

Constraint legend: (1) cross-process bytecode, (2) built-in sandbox, (3) no user-defined fn / closures, (4) no global mutable state.

## Why a custom VM (the four hard constraints)

### 1. Bytecode must be serialisable across processes

Controller compiles DSL source on plugin reload (`Compiler::compile_with_source` → `compile_dsl_source` → base64-encoded JSON `CompiledScript`); Gateway workers only receive the bytecode via gRPC config sync and execute it. The Controller is *the* trust boundary for compilation; Gateway never parses raw source. This requires a versioned, deterministically-serialisable bytecode (current `CompiledScript` is `version: u16` + Vec of `OpCode` + constant pool + `source_map` RLE — all serde-derived, see `lang/bytecode.rs:252-267`). None of Rhai / mlua / Boa offers this as a first-class contract; rune comes closest but does not promise cross-version compatibility.

### 2. Sandbox limits must live inside the engine, not the host wrapper

`Validator` (`lang/validator.rs`) runs *before* bytecode reaches the Gateway VM and rejects jumps-out-of-bounds, oversized maps / lists, unsafe `LoadConst` types, regex pre-compilation past 1 MiB DFA, and wrong-arity builtin calls. `VmLimits` enforces step / loop / call budgets at runtime. Host-wrapper-only sandboxes (the Rhai / mlua / rune approach) leak through whenever a contributor forgets to gate one new feature; in-engine validation rejects the bytecode before it can be loaded.

### 3. No user-defined fn, closures, or module system

Three reasons, in priority order:

- **Attack surface**: every closure or module gives an operator-supplied script a capability to construct a payload that escapes simple static analysis. Without them, the bytecode is straight-line + structured loops + builtin calls, and `Validator` can prove its bounds.
- **Bytecode compactness**: a function table and closure-capture machinery push `CompiledScript` from ~hundreds of bytes (typical) into the kilobyte range, increasing gRPC sync payload and Controller→Gateway sync latency.
- **Use-case scope**: real-world DSL scripts in production are <50 lines. A function call's only benefit is reuse across scripts; the project's plugin-config separation already provides that (declare a second `EdgionPlugins` entry with the shared code).

### 4. No global interner / no shared mutable state

`Value` is owned per `VmState`; no global `Arc` cache, no thread-local cache, no `static` mutables. This makes per-request execution embarrassingly parallel and avoids the "global mutex held under load" failure mode that Rhai's package system and mlua's GC have surfaced in production reports. Pre-compiled regex caches are per-`Vm`, not per-process.

## Rhai deep-dive (what was borrowed, what was rejected)

### Borrowed (✅ adopted into EdgionDSL)

- **Async execution shape** — Rhai's `eval_async_with_scope` pattern informed `Vm::execute` returning an awaitable; HTTP / Redis builtins use `tokio::task::spawn` only when truly needed, otherwise `.await` in place.
- **Step / call / string triple** — Rhai's three-budget model (`max_operations`, `max_call_levels`, `max_string_size`) maps directly to `VmLimits.max_steps`, `max_call_depth`, `max_string_len`.
- **Panic isolation** — `execute_safe` wraps `execute` in `catch_unwind`-style boundary, returning `RuntimeError::Internal` instead of crashing the worker. Rhai's `EvalAltResult::ErrorRuntime` shape inspired the kind-only `RuntimeError` variants.
- **Regex pre-compile** — Rhai's per-AST regex compilation cache informed `Vm::new`'s `Constant::Regex` re-compile step (`vm.rs:201-215`). Bounded `RegexBuilder` with `size_limit(1<<20)` is EdgionDSL's tightening of the Rhai default.

### Rejected (❌ not adopted)

- **AST-as-runtime** — Rhai walks an `AST`; EdgionDSL compiles down to flat `OpCode` for serialisability and tight validation. Tree-walking AST cannot satisfy constraint 1.
- **`OptimizationLevel::Full`** — Rhai's constant-folding / dead-code elimination happens at runtime per-call; EdgionDSL's `Compiler` is the only optimiser, runs once at Controller load, and emits canonical bytecode.
- **Packages (`def_package!`)** — package registration is runtime-mutable and requires `Arc<Mutex<Map>>` lookups per call. EdgionDSL uses a closed `BuiltinId` enum (`lang/bytecode.rs:116`) with `match`-dispatch in `call_builtin`. See [architecture/builtin-dispatch-non-exhaustive-lint.md](architecture/builtin-dispatch-non-exhaustive-lint.md) for why the lint-based safeguard there was also rejected.
- **`Module` / `import`** — see constraint 3.
- **`Dynamic`** — Rhai's tagged-union with `TypeId` and `Box<dyn Any>` payload. EdgionDSL's `Value` enum is 24 B by construction (see [dsl-value-str/map-variant-is-boxed.md](dsl-value-str/map-variant-is-boxed.md)) and excludes user-extensible types entirely. The `assert_value_size_invariant` test locks the 24 B contract.
- **Closures with capture** — same reason as user-defined fn; capture lists would need either GC or a captured-value table per script, both at odds with constraint 4.

## mlua / rune / Boa — single-row verdicts

- **mlua (Lua 5.1/5.4 + LuaJIT bindings)** — rejected on constraints 1 (no cross-process bytecode), 2 (sandbox is host-side `debug.sethook`), 3 (full Lua including metatables), 4 (Lua GC is thread-local mutable). Maintenance state of the underlying Lua engines is excellent; the constraints alone disqualify the option.
- **rune** — rejected primarily on constraint 3 (first-class closures / async fn) and project-maturity (frequent breaking releases, no LTS branch). The compiler-to-`Unit` pipeline is closest to EdgionDSL's Compiler→Bytecode shape and is worth revisiting if rune adopts a sandbox / no-closure mode in a stable release.
- **Boa (JS in pure Rust)** — rejected on constraints 1 (tree-walking interpreter), 2 (inherits ECMAScript's `eval` / prototype-pollution surface), 3 (full JS function space). Useful as a reference implementation for spec-driven language design but not viable as Gateway runtime.

## Rejected internal alternatives (reverse-links)

- **`Dynamic` enum + inline `Box<Map>` wrapping** → rejected; see [dsl-value-str/map-variant-is-boxed.md](dsl-value-str/map-variant-is-boxed.md). `Value::Map(Box<IndexMap>)` is intentional, shrinking `size_of::<Value>()` from 72 B → 24 B; unboxing proposals are blocked by the `assert_value_size_invariant` test.
- **`ImmutableString` / SSO for `Value::Str`** → deferred indefinitely; see [dsl-value-str/sso-migration-not-worth-it.md](dsl-value-str/sso-migration-not-worth-it.md). Four candidate crates evaluated; outer-`Arc` defeats SSO for `SmartString`/`CompactString`; `kstring` 2.0.2 source-confirmed non-zero-copy `into_string`; net realised benefit too small to justify dependency + MSRV lift.
- **Source-position errors at runtime** → deferred-gated; see [observability/dsl-error-line-number-declined.md](observability/dsl-error-line-number-declined.md) and `tasks/todo/dsl/03-runtime-fault-at-line.md` (still open under three gating conditions). `RuntimeError` kind + embedded `CompiledScript.source` + offline disassembly cover the realistic operator need.
- **`tracing::*` on the DSL runtime path** → declined; see [observability/dsl-vm-execute-tracing-declined.md](observability/dsl-vm-execute-tracing-declined.md). Data-plane zero-tracing iron rule applies; PluginLog + offline disassembly is the supported developer channel.
- **`#[export_module]` macro-based builtin registration** (Rhai style) → declined; the closed `BuiltinId` enum with explicit `match`-dispatch gives ~0 ns dispatch overhead, compiler-checked exhaustiveness within each `match`, and a single `LoadConst → CallBuiltin` opcode pair. See [architecture/builtin-dispatch-non-exhaustive-lint.md](architecture/builtin-dispatch-non-exhaustive-lint.md) for the lint-based protection that was *also* rejected (real protection is removing `_ =>` catch-alls).
- **`cargo-fuzz` target for bytecode deserialisation** → replaced by proptest under `tests/property_bytecode.rs`; see [deserialization/dsl-bytecode-deserialize-safety.md](deserialization/dsl-bytecode-deserialize-safety.md). proptest covers ~80% of cargo-fuzz value at stable-toolchain / zero CI-config cost; cargo-fuzz becomes worthwhile only if the threat model flips from *Controller-trusted* to *user-untrusted* (e.g., a plugin marketplace).

## Re-investigation requests that are NOT accepted

Fix suggestions of the following forms are **not accepted**:

- **"Try Rhai again with `OptimizationLevel::Simple` and a host sandbox wrapper"** — constraints 1 and 2 are not negotiated by optimisation level; the wrapper-based sandbox failure mode (one forgotten gate = escape) is the whole reason the validator lives in-engine. Reading the seven-report investigation is the prerequisite, not the action.
- **"Add closures, but only when the bytecode passes a static-analysis pass that proves no capture escapes"** — the static-analysis pass would itself become attack surface (analyser bugs admit unsound captures) and bytecode size would grow even when closures aren't used. The simpler invariant is: no captures, full stop.
- **"Add user-defined fn but disallow recursion via `max_call_depth: 1`"** — defeats the only justification for user-fn (reuse) and creates a confusing language design ("functions but not really"). The plugin-config-as-module pattern is the supported substitute.
- **"Switch to wasm so bytecode is portable to non-Rust runtimes"** — Gateway is the only runtime; portability is a non-requirement. wasm tooling overhead (wasm-bindgen, wasmtime / wasmer host) dwarfs the current bytecode-distribution payload, and wasm sandbox is at the function-import boundary, not the bytecode-shape boundary, so constraints 2 and 3 still need a separate validator on top.
- **"Use Rhai's serde-bytecode serializer for cross-process distribution"** — Rhai explicitly disclaims cross-version compatibility for `AST` serialisation; EdgionDSL's `BYTECODE_VERSION` + `LEGACY_BYTECODE_VERSION` handshake (with the `version` field default and the unsupported-version reject path in `deserialize_base64`) provides the contract Rhai does not.
- **"Add a `--inspect` REPL / debugger"** — out of scope. Developer debugging is served by the offline-disassembler design that consumes `CompiledScript.source` + the runtime PC; gateway-runtime introspection is forbidden by the data-plane-zero-tracing rule. Local debugging belongs in the disassembler CLI (planned, not yet built).

## Re-evaluation triggers

Re-open the engine-selection decision only if **any** of the following is observed:

1. **A new constraint-clearing engine appears** — an off-the-shelf Rust scripting engine ships stable, cross-version bytecode + in-engine sandbox + no user-fn + no global mutable state simultaneously. None of Rhai / rune / mlua / Boa is on that trajectory as of 2026-05-15; revisit only on a concrete release announcement.
2. **The plugin-marketplace use case is approved** — if EdgionDSL is ever opened to third-party script submission (vs the current Controller-trusted model), the threat model flips and the cost calculus of every constraint changes. Constraint 1 still holds; constraints 2/3/4 become even harder, not easier.
3. **A profiling-driven hot spot** in the custom VM's interpreter loop (`Vm::execute` `match opcode`) shows >20% of gateway CPU under realistic load AND a JIT-backed engine (Rhai-jit, mlua-LuaJIT) demonstrably solves it on a representative workload. The current `match`-dispatch loop has never measured as a bottleneck; rebut the assumption with profile data before re-opening.
4. **The maintenance cost of the custom compiler / validator** exceeds the cost of integrating an off-the-shelf engine plus a custom validator on top (judgement call; document the cost comparison before re-opening).
5. **A Rust-language change** makes a constraint trivially solvable in an off-the-shelf engine (e.g., a stable `#[no_implicit_capture]` attribute that disables Rhai closures globally without a fork). At that point the constraint-3 calculus changes.

## Reverse index — `dsl-vm-rhai-research` series → current home

The 7-report investigation series (sub-reports deleted 2026-05-19; this section is the canonical reverse-link):

| Series item | Outcome | Current home |
|-------------|---------|--------------|
| `01-value-size-check.md` | Map boxing locked at 24 B | [dsl-value-str/map-variant-is-boxed.md](dsl-value-str/map-variant-is-boxed.md) |
| `02-list-map-cap-audit.md` | `max_list_len` / `max_map_size` enforced consistently | [`dsl-engine-security-model.md`](../01-architecture/02-gateway/dsl-engine-security-model.md) "Point 02" section |
| `03-source-position-errors.md` §5.1 | `CompiledScript.source` field landed | `lang/compiler/mod.rs` `compile_with_source` + `compile_dsl_source` |
| `03-source-position-errors.md` §5.2 | Runtime line-number access-log declined | [observability/dsl-error-line-number-declined.md](observability/dsl-error-line-number-declined.md) + `tasks/todo/dsl/03-runtime-fault-at-line.md` (gated) |
| `04-builtin-registration.md` | Closed `BuiltinId` enum kept; non-exhaustive lint rejected | [architecture/builtin-dispatch-non-exhaustive-lint.md](architecture/builtin-dispatch-non-exhaustive-lint.md) |
| `05-test-coverage.md` §6.2 ROI #1 | proptest target landed | `src/core/gateway/plugins/http/dsl/tests/property_bytecode.rs` + [deserialization/dsl-bytecode-deserialize-safety.md](deserialization/dsl-bytecode-deserialize-safety.md) |
| `06-value-str-migration.md` | SSO migration deferred indefinitely | [dsl-value-str/sso-migration-not-worth-it.md](dsl-value-str/sso-migration-not-worth-it.md) |
| `07-rhai-feature-shopping-list.md` | Engine-selection rationale frozen | **this file** |
| `issue-10-fill-compiled-script-source.md` | Phase 0 implemented | `lang/compiler/mod.rs:99-101` (`compile_with_source`) |
| `issue-11-parser-span-propagation.md` | Deferred with the line-number decline | [observability/dsl-error-line-number-declined.md](observability/dsl-error-line-number-declined.md) §"Related declined-but-distinct work" |
| `issue-12-source-map-rle.md` | Partial: data structure lives in bytecode; access-log consumer declined | `lang/bytecode.rs:261-267` (`source_map` field + `line_of` reader) |
| `issue-13-runtime-error-line-in-access-log.md` | Declined | [observability/dsl-error-line-number-declined.md](observability/dsl-error-line-number-declined.md) |
| `issue-14-enable-non-exhaustive-lint.md` | Lint rejected; explicit-list rewrite landed instead | [architecture/builtin-dispatch-non-exhaustive-lint.md](architecture/builtin-dispatch-non-exhaustive-lint.md) |
| `issue-19-deserialize-base64-fuzz.md` | Replaced by proptest equivalent | `tests/property_bytecode.rs` + [deserialization/dsl-bytecode-deserialize-safety.md](deserialization/dsl-bytecode-deserialize-safety.md) |

## Reference cases

- `tasks/todo/dsl/` `README.md` — the four-item priority matrix that survived the investigation; 01 / 02 closed 2026-05-19, 03 gated, 04 = this file.
- `tasks/todo/dsl/03-runtime-fault-at-line.md` — gated runtime-line wrapper proposal; do not implement until one of its three gating conditions fires.
- [`../01-architecture/02-gateway/dsl-engine.md`](../01-architecture/02-gateway/dsl-engine.md) — architectural overview (data flow, source layout, plugin entry points, async/sync bridging, panic isolation, runtime lifecycle).
- [`../01-architecture/02-gateway/dsl-engine-security-model.md`](../01-architecture/02-gateway/dsl-engine-security-model.md) — sandbox cap enforcement, ReDoS posture, SSRF guard rationale.
- `src/core/gateway/plugins/http/dsl/lang/` — the custom VM source tree, ~7k lines of safe Rust including inline tests.
- User-facing docs (out of scope for this internal ADR): `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/`.
