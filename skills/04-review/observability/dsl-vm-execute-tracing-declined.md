---
name: dsl-vm-execute-tracing-declined
description: Use when reviewing proposals to add `tracing::*` (any level, including `debug!`) anywhere on the DSL `Vm::execute` / `execute_safe` runtime-error path. The proposal is declined; the data-plane zero-tracing iron rule applies.
---

# DSL `Vm::execute` runtime-error tracing — DESIGN-TRADEOFF (DECLINED)

## Conclusion

Adding **any** `tracing::*` macro — including `tracing::debug!(target: "edgion::dsl", ...)` — inside `Vm::execute`, `execute_safe`, or any helper they call (`call_builtin_http`, `call_builtin_redis`, `call_builtin`, etc.) is **not adopted**. The runtime-error path keeps to `log.push_err(&'static str)` only.

Fix suggestions of the form "emit one `tracing::debug!(pc=..., err=%runtime_err)` on the `Err` branch of `execute_safe` so developers can debug DSL crashes locally" are **not accepted**, even when the proposal sets `target: "edgion::dsl"` to gate operator visibility.

## Core Rationale

**1. `Vm::execute` is on the data-plane hot path — `tracing` is forbidden regardless of level**

`skills/03-coding/01-log-safety.md` Iron rule 3 ("zero tracing on the data plane") explicitly lists `plugins/http/**/plugin.rs`'s `filter` / `apply` / `on_request` methods as data plane, and `Vm::execute` is invoked from `DslPlugin::filter` via `execute_safe`. The iron rule applies to **every** level (`debug!` is called out by name), not just `info!` / `warn!` / `error!`. The reasoning (format overhead under load, log explosion at QPS × per-request emission rate, lack of request correlation, duplication of access-log information) all hold regardless of `target=` or `level=`.

**2. The `target=` gate is operator-visible defense in depth, not a path-class exemption**

`tracing::debug!(target: "edgion::dsl", ...)` reduces operator-side visibility (`EnvFilter` default `info` skips it), but does **not** bypass the format-overhead and request-correlation arguments. The `target=` argument is a content-control mechanism, not a path-class one — it cannot move `Vm::execute` out of the data-plane entry list documented in `01-log-safety.md`.

**3. Acceptable alternatives already exist**

For the legitimate developer-debug use case ("when a DSL script crashes at PC N, I want to map N back to source"), the project has these channels:

| Need | Channel |
|---|---|
| Per-request decision / failure kind | `PluginLog::push_err(&'static str)` → access log (already in place) |
| Aggregate runtime-error counts | `global_metrics().xxx()` increment per `RuntimeError::*` variant |
| PC → source correlation | `CompiledScript::source` field (Phase 0) + offline disassembly tool (developer-side, not gateway-runtime) |
| Source line numbers for runtime errors | **Declined separately** — see [dsl-error-line-number-declined.md](dsl-error-line-number-declined.md) |

The PC value at error time can be reconstructed offline from the embedded `CompiledScript.source` + a dedicated disassembler/debugger; it does not require runtime gateway-side log emission.

**4. The compile path is different — `tracing` there is OK**

`Compiler::compile_with_source` / `compile_dsl_source` / `DslPlugin::build_from_config` run only on plugin reload (ConfHandler dispatch), not per-request. Adding `tracing::info!` / `tracing::error!` there is permitted under the existing config-load exception. The constraint is specifically on `Vm::execute` and downstream.

## Fix Suggestions Not Accepted

- **"Add `tracing::debug!(pc=state.pc, err=%runtime_err)` to `execute_safe`'s `Err` branch"** — directly violates Iron rule 3; `execute_safe` is called from `DslPlugin::filter`, on the data-plane hot path. The `target: "edgion::dsl"` gate does not move the call site off the hot path.
- **"Split `Vm::execute` into an outer shell + inner `execute_loop` so the outer shell can read `state.pc` and emit `tracing::debug!` on error"** — the split itself is harmless, but if its only purpose is to enable a forbidden `tracing::debug!`, the split is dead refactoring; reject the entire change. (If the split has a separate motivation, evaluate independently.)
- **"Use `tracing::trace!` instead of `debug!` — even less visible by default"** — same iron-rule violation; trace level is still subject to the format-overhead and per-request emission arguments. Iron rule 3 applies to all `tracing::*` macros without level exception.
- **"Emit a `tracing::debug!` only when the process is built with `cfg!(debug_assertions)`"** — production gateway runs release builds, so this is dead code in production; the proposal does not deliver the stated developer-debug value (developers usually need this on a deployed gateway, not a local cargo-test run). Also disagrees with the data-plane discipline: the rule does not have a debug-build exemption.
- **"Add a side-channel `RuntimeErrorChannel` via `tokio::sync::mpsc` that emits to a separate file"** — reinvents a logging stack; same data-plane hot-path emission cost; consumes per-script state. The simpler / approved channel is `PluginLog::push_err` + the offline disassembler.

## Re-evaluation Triggers

Re-open this decision only if **any** of the following is observed:

- A concrete production incident where developers provably could not diagnose a DSL crash from `RuntimeError` kind + `CompiledScript.source` + offline disassembly, AND there is no path to extend the existing access-log / PluginLog channel to carry the needed signal.
- Iron rule 3 itself is amended (e.g., a new "debug-only target prefix" exception is added to `01-log-safety.md`). The amendment must come first; piecewise carve-outs at each call site are not accepted.
- A type-system mechanism emerges that makes `tracing::*` on a marked data-plane function a compile-time error, **and** the proposal adds the channel via that mechanism (i.e., the data-plane discipline becomes statically enforced rather than review-enforced). At that point a controlled exception path can be designed.

## Reference Cases

- `tasks/todo/dsl-vm-rhai-research/issues/issue-10-fill-compiled-script-source.md` — Phase 0 of the parent report `03-source-position-errors.md` §5.1. The `source` embedding half landed; the `tracing::debug!` half was declined under this rule.
- `skills/03-coding/01-log-safety.md` — Iron rule 3 (zero tracing on the data plane), Plugin dispatch entry list.
- `skills/04-review/observability/observability-review-rule.md` — Iron Rule: Zero Tracing on the Data Plane (review-side checklist).
- `skills/04-review/observability/dsl-error-line-number-declined.md` — sibling decision; the `push_err(&'static str)` invariant rationale also forbids smuggling dynamic content via PluginLog.
- `src/core/gateway/plugins/http/dsl/lang/vm.rs` — `Vm::execute` body; `execute_safe` `Ok(Err(runtime_err))` arm (the `log.push_err(kind)` line is the boundary, not a launching point).
- `src/core/gateway/plugins/http/dsl/plugin.rs` — confirmed-OK `tracing::error!` call sites at lines ~201/208/219/226 are all on the plugin-construction / VM-init path (config load, not data plane); those are not affected by this rule.
