---
name: dsl-validate-construct-double-resolve
description: Use when reviewing findings that DSL plugin bytecode is deserialized + validated twice per hot-reload (validate_config and resolve_parts both call resolve_script); covers why the two-path self-validation is an accepted tradeoff and which de-duplication fixes are not accepted.
knowledge_type: intentional-design
---

# DSL plugin validate-then-construct double `resolve_script` — DESIGN-TRADEOFF (ACCEPTED)

## Conclusion

Each enabled `DslPlugin` runs `edgion_dsl::resolve_script` (base64 decode + JSON deserialize + full `Validator` scan) **twice per config apply / hot-reload**: once in the control-plane validation gate (`DslConfig::validation_error` → `DslConfig::compile_and_validate`) and once in the data-plane constructor (`DslPlugin::from_config` / `from_config_response` → `resolve_parts` → `DslConfig::compile_and_validate`). This is **redundant work, accepted as-is**. The two-path self-validation is intentional defense-in-depth, the redundancy is off the per-request data-plane hot path, and every safe de-duplication fix is disproportionate to a Low-severity redundant-work finding.

Fix suggestions of the form "return the `CompiledScript` from `validate_config` and thread it into construction", "skip validation in `from_config` because `validation_error` already ran", or "cache / memoize the `CompiledScript`" are **not accepted**.

## Core Rationale

**1. Two-path self-validation is intentional defense-in-depth, not an accident**

`add_from_request_filters` (`edgion-gateway/src/plugins/runtime/plugin_runtime.rs:391`) and `add_from_upstream_response_filters` (`:476`) call the generic `entry.plugin.validation_error()` gate for **every** plugin type uniformly — its non-empty result records the plugin in `invalid_plugins`, surfaces the error, and feeds the fail-closed seal. Construction happens later and separately (`:420`/`:755` for request, `:501`/`:778` for upstream response). `DslPlugin::from_config` (`edgion-gateway/src/plugins/http/dsl/plugin.rs:80`) is a `pub` constructor that **must validate independently** — it cannot assume the gate ran first. The doc comment on `DslConfig::compile_and_validate` (`edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs:143-153`) states this explicitly: it exists so the control plane sees "the exact set of failures the gateway's `DslPlugin` constructor would hit". Removing the `Validator` pass from either path lets unvalidated bytecode reach `Vm::new` when the entry point is called standalone — a security/contract regression. The security property is sound precisely because both paths validate.

**2. The redundancy is off the data-plane hot path**

`resolve_script` runs only on config apply (operator changes an `EdgionPlugins` resource → `PluginRuntime` rebuild), not per request — the constructed `Vm` is cached and `run_request` reuses it. Realistic DSL scripts are <50 lines and bounded by `ValidationLimits`; base64 decode + JSON deserialize + a single linear `Validator` opcode/constant scan is sub-millisecond, and doubling a sub-millisecond operation at config-apply frequency is invisible. The data-plane prohibition that drives most DSL review rules does not apply here because this code never executes per request.

**3. No low-cost safe fix exists**

The validation gate (`:391`) is plugin-agnostic and DSL-specific local state does not survive from it to the construction dispatch (`:755`), so there is no place to hand the already-resolved `CompiledScript` through without restructuring the shared loop. The available de-duplication routes each carry cost/risk out of proportion to a Low redundant-work finding (see below).

## Fix Suggestions Not Accepted

- **"Return `CompiledScript` from `validate_config` and pass it into `resolve_parts`"** — the validation gate and the constructor are non-adjacent call sites in a generic per-plugin loop; there is no DSL-scoped local that survives between them. Threading the artifact through would require changing the shared `PluginValidation` trait (a core public abstraction used by all plugin configs) — a wide-impact change rejected for a Low finding.
- **"Skip validation in `from_config` since `validation_error` already ran"** — breaks `from_config`'s self-contained safety contract (`pub`, may be called standalone); unvalidated bytecode could then construct a VM.
- **"Cache `CompiledScript` keyed by config identity at the `plugin_runtime` layer"** — introduces a new cache with eviction/invalidation correctness concerns (a memory-leak review surface) to save microseconds on a non-hot path.
- **"Memoize `CompiledScript` inside `DslConfig` via `OnceCell`"** — changes the config type's `Clone` / `PartialEq` / serde semantics; `DslConfig` is cloned and compared across the controller→gateway sync path.

## Re-evaluation Triggers

Re-open only if **any** of the following is observed:

- Profiling shows `resolve_script` as a measurable fraction of an `EdgionPlugins` config-apply for production-scale scripts (i.e. config-apply latency becomes a real bottleneck).
- DSL scripts routinely grow to many hundreds of opcodes/constants such that the double `Validator` scan is measurable.
- A refactor unifies the plugin validate+construct lifecycle into a single call site for all plugins — at that point threading the resolved artifact becomes free and the redundancy should be removed.

## Reference Cases

- Finding `plugins-http-dsl-F010` (2026-05-26) — closed as accepted tradeoff.
- `edgion-gateway/src/plugins/http/dsl/plugin.rs` — `resolve_parts`, `from_config`, `from_config_response`.
- `edgion-dsl/src/script.rs` — `resolve_script`, `check_redis_link_names`, `is_single_push_op`.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs` — `DslConfig::validation_error`, `DslConfig::compile_and_validate`.
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` — the generic validate gate vs construct dispatch.
- [plugin-runtime-god-match.md](../../plugin-runtime/intentional-design/plugin-runtime-god-match.md) — related plugin-runtime construction design decisions.

## Resolution references

- `plugins-http-dsl-F010` — 2026-05-26 (⚠️ accepted tradeoff).
