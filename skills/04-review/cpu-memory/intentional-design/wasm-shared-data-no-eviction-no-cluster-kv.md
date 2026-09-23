---
name: wasm-shared-data-no-eviction-no-cluster-kv
description: Use when reviewing wasm shared data storage-policy findings; covers why no LRU/TTL eviction, no Redis/cluster backend, no cross-reload retention, and no restart persistence are accepted design decisions.
status: accepted-tradeoff
finding_id: wasm-shared-data-storage-analysis
closed: 2026-07-12
knowledge_type: intentional-design
---

# Wasm shared data: no eviction, no cluster KV backend, process-local lifetime — DESIGN-TRADEOFF

## Conclusion

The wasm shared data store (`edgion-gateway/src/plugins/http/wasm/shared_data.rs`) deliberately
has **no LRU/TTL eviction**, **no Redis/cluster-level backend**, **no guaranteed cross-rebuild retention**, and **no restart persistence**. Fill-up is handled
fail-loud: writes beyond the 4MiB per-store cap return `TooLarge` and the guest decides.

LRU/TTL eviction, transparent Redis backing, and restart persistence are outside
this accepted storage model. Reconsider them against the concrete triggers below;
the historical decision alone does not disprove a new defect or requirement.

## Core Rationale

- **Spec and reference hosts define no eviction.** The proxy-wasm spec leaves shared data
  scope, lifetime, limits, and eviction entirely unspecified (only CAS is normative). Envoy's
  implementation (proxy-wasm-cpp-host `src/shared_data.cc`) has NO size limits, NO eviction,
  NO TTL — data lives until the vm_id's last VM handle is destroyed. Our three caps
  (64KiB/value, 4MiB/module, host-side allocation bounds) are already strictly tighter than
  the reference implementation.
- **Eviction changes guest-visible CAS behavior.** The historical reference hosts
  did not evict shared-data keys; the spec gives guests no eviction notification
  and no defined CAS semantics for a reborn key. The dominant workload (tick-fetched ruleset
  published for request hooks) would turn LRU into "denylist silently half-disappears" — a
  worse failure than a loud `TooLarge`.
- **Cluster KV has no standard-ABI answer.** Envoy/Istio provide none. The only industry
  precedent, Higress, added `DispatchRedisCall`-style host functions in a forked SDK —
  a non-portable ABI extension. Edgion's division stands: process-local = shared data;
  cluster-level = `rate_limit_redis` native plugin or guest callouts (tick-callout is
  implemented and covers the poll-and-publish pattern). A Higress-style host extension is
  deferred until a real user scenario demands direct guest KV access, not rejected forever.
- **Store lifetime follows the private loaded entry of a configured plugin.** The
  user-approved runtime redesign gives each constructed `WasmPlugin` a private
  pool/shared-data/tick scope. Common `PluginGeneration` / `EntryCache` Arc reuse
  shares one plugin across route and policy views; distinct stage/index entries
  and resource generations do not share data even with identical bytecode.
  Rebuilding configuration may reset data and repeat compilation. OCI refresh
  publishes a fresh entry and store, while old request leases retain their old
  entry until completion. This supersedes content-identical global cache reuse
  and the 60-second eviction grace. Owner destruction explicitly cancels ticks;
  a tick cannot postpone cancellation by retaining the entry itself. See
  [runtime isolation](../../wasm/regression-guards/wasm-module-cache-key-runtime-config.md).
  The earlier Envoy leak precedent (envoy#13835 / cpp-host#86, fixed by #114)
  remains historical evidence for keeping store lifetime tied to live execution
  state, not authority for a particular global cache or cross-plugin identity.

The user explicitly accepted state reset and repeated cold compilation to simplify
ownership and ensure final configuration correctness. This is a scoped design
choice, not a universal requirement that all caches use instance identity.
The replacement passed all 226 selected Gateway Wasm tests on 2026-09-23,
including independent stores, configuration replacement, and retained request leases.

## Explicitly rejected alternatives

- LRU or TTL eviction: no spec, no precedent, correctness risk exceeds memory benefit.
- Transparent Redis backend for shared data: network latency and availability coupling on the
  request hot path, CAS→WATCH/Lua mapping, semantic break with every existing guest.
- Cross-restart persistence / cross-reload identity: no host precedent; stale-data and
  format-migration hazards exceed the cold-start gap that tick fetch already fills.

## Related historical correction

Accounting originally counted only logical key+value bytes, excluding ~100–150B/entry
structural overhead, so pathological many-tiny-key loads could inflate the 4MiB cap's real
RSS by orders of magnitude. **Fixed 2026-07-12**: `ENTRY_OVERHEAD_BYTES = 128` is charged per
entry on insert/delete (`shared_data.rs`), bounding entry count to ~30k and making the cap
track real memory. Recheck that accounting before treating the old overhead gap
as evidence for eviction; a fresh defect remains reportable.

## Re-evaluation triggers

- A real user scenario where guests need direct cluster KV and callout polling is insufficient
  (revisit Higress-style host extension, using their function signatures as the benchmark).
- The proxy-wasm spec gains normative eviction/TTL semantics.
- Evidence of a production workload where fail-loud `TooLarge` at 4MiB is operationally worse
  than eviction (would need guest-visible eviction semantics first).

## Reference cases

- Analysis and sources: `tasks/wasm-followups/shared-data-storage-analysis.md` (2026-07-12).
- Envoy leak precedent: envoyproxy/envoy#13835, proxy-wasm/proxy-wasm-cpp-host#86 (fixed #114).
- Sibling multi-tenant trust note: `docs/en/.../wasm.md` shared data section (tick-fetched
  data readable by all references sharing the same constructed plugin).
