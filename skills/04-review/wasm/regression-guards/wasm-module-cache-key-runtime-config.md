---
name: wasm-module-cache-key-runtime-config
description: Use when reviewing Wasm module-cache findings; covers what the runtime cache key must fold in so per-plugin config is not cross-contaminated.
status: fixed
finding_id: contract-01
closed: 2026-07-18
knowledge_type: regression-guards
---

# Wasm module cache key must fold in per-plugin runtime config — FIXED

## Conclusion

The process-wide Wasm module cache (`WasmVmManager`) keys entries by
`runtime_cache_key(config)`. A cached `ModuleEntry` owns not just compiled
bytecode but the per-plugin runtime fields `instance_pool_size`, `timeout_ms`,
`vmConfig`, and `pluginConfig`. The runtime key therefore MUST fold a digest of
exactly those four fields in addition to the bytecode source identity. Without
it, two plugins sharing one module source but differing in any of those fields
alias the first-loaded entry (cross-route isolation failure + silent no-op on a
configuration-only hot reload). Fixed 2026-07-18.

Re-reports of the "cache keyed only by bytecode source / different `pluginConfig`
share one entry / config-only hot reload has no effect" form are **not accepted**.

## Core Rationale

**1. Three distinct key/identity roles — do not conflate them**

- `WasmSourceSpec::cache_key()` (`edgion-resources/.../wasm.rs`) = schema-level
  **source identity** only (inline SHA-256, file path, url, oci image). It is
  intentionally config-independent and must stay that way.
- `runtime_cache_key()` (`edgion-gateway/.../wasm/vm_manager.rs`) = the actual
  cache key. It extends the source key with, for `file` sources, the file's
  `size:mtime` (`;stat=`), and for **all** sources a digest of the per-plugin
  runtime fields (`;rt=`), via `wasm_runtime_config_digest`.
- `TickCalloutState` = a per-key refcounted registry that merges
  `calloutAllowlist` / `calloutTimeoutMs` across plugins on the same key.

**2. `ModuleEntry` carries per-config state, so the key must separate configs**

`load_inner` copies `instance_pool_size → pool_cap`, `vm_config`,
`plugin_config`, and `timeout_ms → hook_timeout_ms` into `ModuleEntry`, and
`new_vm` drives `proxy_on_vm_start` / `proxy_on_configure` from the cached
vm/plugin config. A source-only key let the first plugin to load a source impose
its configuration and instance pool on every other plugin using that source.

**3. Callout fields are deliberately excluded from the key**

`calloutAllowlist` / `calloutTimeoutMs` are merged per-key by `TickCalloutState`,
not stored as per-entry config, so two plugins differing only in callout config
should still share the entry. Folding them into the key would fragment the
callout registry. The digest covers exactly the four `ModuleEntry` config fields
and nothing else. `failOpen` lives on `WasmPlugin` (not `ModuleEntry`) and is
likewise not part of the key.

## Fix Suggestions Not Accepted

- "Also key on `calloutAllowlist` / `calloutTimeoutMs`" — those are registry-merged
  on purpose; keying on them breaks the tick-callout aggregation.
- "Compare the full `WasmConfig` inside `get_or_load` instead of widening the key" —
  the key is computed once at `WasmPlugin::build` and reused by preload / sync
  `try_get` / refcount; an in-`get_or_load` compare would diverge from the key the
  plugin registered under.
- Re-report proposing the larger "separate compiled `Module`/`InstancePre` cache
  from a per-config runtime entry/pool" refactor — that is a wide-impact redesign,
  not a narrow review fix; the digest-key correction closes the correctness defect.
  Promote to `tasks/todo/` via brainstorming only if the double-compile cost of
  same-module/different-config deployments is ever measured to matter.

## Re-evaluation Triggers

Re-open only if: a new per-plugin field is added to `ModuleEntry` from the config
and is NOT folded into `wasm_runtime_config_digest`; or the compilation cost of
compiling one module once per distinct config becomes a measured hot-path concern
(then the compiled-artifact/runtime split becomes worth its complexity).

## Reference Cases

- Finding `contract-01` (review-wasm-issues), fixed 2026-07-18.
- `edgion-gateway/src/plugins/http/wasm/vm_manager.rs` — `runtime_cache_key`,
  `wasm_runtime_config_digest`, `ModuleEntry`, `load_inner`, `get_or_load`.
- Regression tests: `runtime_cache_key_separates_runtime_config_but_not_callouts`,
  `same_source_distinct_plugin_config_do_not_alias`,
  `plugin_config_only_reload_lands_new_entry` (same file).
- Related wasm cache decisions: [`../cpu-memory/cases/wasm-shared-data-no-eviction-no-cluster-kv.md`](../../cpu-memory/intentional-design/wasm-shared-data-no-eviction-no-cluster-kv.md),
  [`../concurrency/wasm-tick-callout-grant-evict-race.md`](../../concurrency/intentional-design/wasm-tick-callout-grant-evict-race.md).

## Resolution references

- `contract-01` — 2026-07-18.
