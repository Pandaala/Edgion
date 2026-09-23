---
name: wasm-plugin-runtime-isolation
description: Review configured Wasm plugin ownership, mutable state isolation, and removal of global runtime-key sharing.
knowledge_type: regression-guards
---

# Wasm runtime isolation follows configured plugin instances

## Conclusion and scope

The user-approved ownership model gives each constructed `WasmPlugin` a private
`WasmRuntime`: load/publication slot, instance pool, shared data, background tick,
and immutable callout grant. Identical module bytes or configuration do not merge
different plugin instances. Request hooks and background ticks use that instance's
grant; there is no cross-plugin grant registry or conflict arbitration.

The common `PluginGeneration` / `EntryCache` owns constructed filters through
`Arc`. Its identity is one immutable resource generation and one `EntrySlot`
(`stage`, `index`). Multiple route/policy views can share that same plugin. Different
entries, stages, resources, and rebuilt generations have independent mutable state.
Existing requests can finish on their captured generation.

The process-wide `WasmVmManager` retains the Wasmtime Engine and epoch ticker.
The existing bounded OCI artifact byte cache remains separate. There is no global
compiled-module/runtime map, runtime cache key, manual plugin refcount, or delayed
module eviction. Warm access reads the plugin's own atomic publication directly.

## Rationale and evidence

Stable issue `wasm-tick-callout-grant-eviction-race` exposed an authorization
consistency defect in the former three-map manager. Eviction removed a zero
refcount, plugin A registered into the old grant state, and eviction then removed
that state. Plugin B could register a different grant into a fresh state and enable
tick dispatch although A remained live. This established a bypass of the former
conflict guard, not access to an origin authorized by no live plugin.

The user explicitly chose instance isolation and eventual cleanup over preserving
cross-plugin sharing and reload intermediate states. Removing that aggregation
mechanism eliminates its grant-loss boundary; it does not reinterpret the old
behavior as safe. The cost is repeated compilation and fresh shared data when a
plugin is reconstructed. No new compiled-artifact cache is added without evidence
that its complexity is needed.

Sources: `plugins/http/wasm/{plugin,vm_manager}.rs`,
`plugins/runtime/entry_bundle.rs`, and `plugins/http/plugin_generation.rs` in
`edgion-gateway/src`. The architecture boundary is documented in
[the plugin system](../../../01-architecture/02-gateway/05-plugin-system.md).
Background destruction is covered by
[the owner shutdown guard](tick-quiescence-on-zero-reference-eviction.md).

## Historical correction

`contract-01` (2026-07-18) corrected a source-only cache that let one plugin's
`instance_pool_size`, `timeout_ms`, `vmConfig`, and `pluginConfig` contaminate
another. Its fix added those fields to `runtime_cache_key`; callout fields remained
in a separate aggregate. That key implementation is superseded by instance
ownership. The enduring outcome is isolation of configuration and mutable runtime
state, not a requirement to retain a global cache or a particular key digest.

## Verification and re-evaluation

On 2026-09-23, `cargo test -p edgion-gateway --lib wasm -- --test-threads=4`
passed all 226 tests, and the `plugins::runtime::task_scope` test passed.
Workspace check and Clippy passed. Independent read-only review returned LGTM.
Coverage includes distinct instances not sharing pools/data/grants, repeated
loads reusing one runtime's entry, configuration replacement isolating generations,
and destruction stopping background work without invalidating request leases.
Read-only review also confirmed common plugin views retain their shared Arc.
Historical tests of key digests do not
verify this replacement design.

Live verification on the same date passed `WasmRuntimeOwnership` (initial grants,
swapped grants, restored configuration, and cleanup), all nine static
`EdgionPlugins/Wasm` tests, and the mandatory cache-residue gate. The latter wiped
35 resources without errors and reported no leaks. Both suites used one shared
Controller/Gateway topology. Formatting, agent-doc, SSA-force, metrics inventory,
Pingora source, Gateway API transform, and Gateway no-tracing checks passed; the
integration runner's English-only, type-purity, and shell preflights also passed.

Re-evaluate if mutable state is shared across instances, a compiled-artifact cache
is introduced, or the common entry cache changes its resource/stage identity.

## Resolution references

- `contract-01`: historical source-only runtime aliasing correction.
- `wasm-tick-callout-grant-eviction-race`: replacement of cross-plugin mutable
  runtime sharing with user-approved plugin instance ownership.
