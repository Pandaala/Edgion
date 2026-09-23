---
name: shared-data-gauge-live-stores
description: Use when reviewing Wasm shared-data byte metrics, overlapping module generations, or cached metric handle lifetimes.
knowledge_type: regression-guards
---

# Shared-data byte gauges aggregate live stores

## Conclusion and scope

`edgion_wasm_shared_data_bytes{module}` sums accounted bytes across every live
`SharedData` with the same canonical source metrics label. Runtime configuration
variants, file reloads, signed URL rotations, and OCI refresh generations can
share a label while owning different stores. An old request lease retains its
entry and store after cache replacement or eviction. The 4 MiB admission cap
remains per store, not per metric series.

## Rationale and evidence

The former `emit_bytes_gauge` set the series to one store's local `total_bytes`,
overwriting other live contributions, and destruction left a stale value.
`SharedData` now retains an exporter Gauge handle registered by the canonical
metrics registry. Successful mutations apply byte deltas; rejected writes and
uncommitted reservations publish nothing. Growth is published while the key's
shard guard is held, before a concurrent delete can subtract it. Destruction
subtracts only the dying store's final total after all mutations have finished.
No per-generation byte ledger, label, or request-path registry lookup is added.

The authority is `skills/00-design-rules.md` sections C and U and
`skills/03-coding/observability/01-metrics.md`: symmetric gauges, canonical
registration, bounded labels, and construction-time metric handles.

## Alternatives and limits

Setting zero on `ModuleEntry::drop` would erase other stores' contributions.
Generation labels would accumulate series. Changing KV identity or lifetime
would violate the separate accepted storage policy in
`../../cpu-memory/intentional-design/wasm-shared-data-no-eviction-no-cluster-kv.md`.

The exporter evicts idle gauges by default. The registry therefore keeps weak
handles by module label and touches live handles before Gateway metric rendering,
following the existing LB counter keepalive boundary. Registration and render
share a cold-path lock; mutation and store destruction never acquire it. Dead
weak entries are pruned on registration, scrape, and store-stats collection.
This metadata is bounded by live labels plus entries retired since the last
cleanup, and is visible as `wasm_shared_data_metrics` in store-stats.

The last drop balances the series to zero; the exporter can subsequently expire
that unowned series. Scrapes during an in-progress mutation may observe a
transitional value, but completed mutations cannot overwrite another store.
This metric accounts key/value bytes and fixed entry overhead, not exact RSS.

## Verification and re-evaluation

Inline `shared_data::tests` covers overlapping stores, both drop orders,
recreation, rejected writes, concurrent mutation/registration/drop, a retained
request-style Arc, and live-handle keepalive with exporter idle eviction enabled.
`vm_manager::tests::shared_data_metric_handle_follows_delayed_module_eviction`
also covers the ownership boundary used by cache-residue validation: the handle
remains visible during the module cache's anti-thrash grace and disappears after
an unleased zero-reference entry is evicted. The validator preserves a nonzero
`wasm_shared_data_metrics.count` as residue. Its 75-second default deadline now
bounds the quiesced eviction path: eviction schedules after the 60-second grace
and then stops the tick task explicitly, so a running tick round no longer
retains a `VmLease` and `SharedData` past the deadline. The residual windows that
path does not cover are enumerated in
[tick-quiescence-on-zero-reference-eviction](tick-quiescence-on-zero-reference-eviction.md).
Existing CAS/capacity tests remain in place. No live integration scenario changes.
Fresh verification: `cargo test -p edgion-gateway --lib shared_data` passed all
21 selected tests, including the real guest ABI roundtrip and preflight checks.
The store-stats serialization test and all three existing LB keepalive tests
also passed. Workspace check/Clippy, formatting, agent-doc consistency, SSA,
metrics inventory, Pingora source policy, no-tracing, English-only, and the
unchanged `edgion-tests` diff guard passed. Clippy retained one pre-existing
`redundant_async_block` warning in the frozen DSL gRPC integration test.
Independent read-only review returned LGTM. These checks do not claim a full
integration-suite run.

For `wasm-shared-data-eviction-grace-exceeds-residue-timeout`, focused current-tree
verification passed the delayed module-eviction metric-handle test, all 23
`cache_residue` tests, the local runner deadline contract test, and the Kubernetes
residue-gate shell test. This focused evidence distinguishes bounded grace from a
nonzero idle-entry signal that survives the validator deadline; it does not claim a
fresh live EdgionPlugins integration sweep. The running-tick-callout case is covered
separately by
[tick-quiescence-on-zero-reference-eviction](tick-quiescence-on-zero-reference-eviction.md).

Re-evaluate if recorder installation order, idle eviction, metrics rendering
entry points, store accounting, or module ownership changes. Metrics must be
installed before constructing instrumented stores; a deliberately rejected
series under the cardinality guard remains subject to that guard's policy.

## Resolution references

- `wasm-shared-data-gauge-generation-lifecycle` — pending task created
  2026-09-07; corrected with mutation deltas and destruction symmetry.
- `wasm-shared-data-eviction-grace-exceeds-residue-timeout` — the residue
  deadline now exceeds the idle module eviction grace, with metric visibility
  and fail-close regression coverage. Running-tick quiescence was closed by
  `wasm-tick-retention-exceeds-residue-deadline` (2026-09-23); its rule lives in
  [tick-quiescence-on-zero-reference-eviction](tick-quiescence-on-zero-reference-eviction.md).
