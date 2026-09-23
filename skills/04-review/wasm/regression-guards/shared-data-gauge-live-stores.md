---
name: shared-data-gauge-live-stores
description: Use when reviewing Wasm shared-data byte metrics, overlapping module generations, or cached metric handle lifetimes.
knowledge_type: regression-guards
---

# Shared-data byte gauges aggregate live stores

## Conclusion and scope

`edgion_wasm_shared_data_bytes{module}` sums accounted bytes across every live
`SharedData` with the same canonical source metrics label. Independent configured plugins, rebuilt
resource generations, signed URL rotations, and OCI refresh generations can
share a label while owning different stores. An old request lease retains its
entry and store after runtime replacement or owner destruction. The 4 MiB admission cap
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
Generation labels would accumulate series. KV identity and lifetime follow the current user-approved storage policy in
`../../cpu-memory/intentional-design/wasm-shared-data-no-eviction-no-cluster-kv.md`.

The exporter evicts idle gauges by default. The registry therefore keeps weak
handles by module label and touches live handles before Gateway metric rendering,
following the existing LB counter keepalive boundary. Registration and render
share a cold-path lock; mutation and store destruction never acquire it. Dead
weak entries are pruned on registration, scrape, and store-stats collection.
This metadata is bounded by live labels plus entries retired since the last
cleanup, and is visible as `wasm_shared_data_metrics` in store-stats. This existing
registry also retains weak module-lifetime metric handles: its metadata count
includes labels with live byte gauges or live module guards. The canonical
`with_live_wasm_metrics` render boundary touches both kinds of handles, including
the release counter, so exporter idle eviction cannot detach a live owner's
construction-time handle. No second metric registry is introduced.

The last drop balances the series to zero; the exporter can subsequently expire
that unowned series. Scrapes during an in-progress mutation may observe a
transitional value, but completed mutations cannot overwrite another store.
This metric accounts key/value bytes and fixed entry overhead, not exact RSS.

## Verification and re-evaluation

Inline `shared_data::tests` covers overlapping stores, both drop orders,
recreation, rejected writes, concurrent mutation/registration/drop, a retained
request-style Arc, and live-handle keepalive with exporter idle eviction enabled.
The plugin-owned runtime replacement removes delayed global module eviction.
An owner drop cancels its background work; outstanding request leases can still
retain their store and metric handle. Store destruction, not clearing another
plugin's metric series, removes its contribution. Multiple private runtimes with
one metrics label are an ordinary case under the new ownership model.
See [owner shutdown](tick-quiescence-on-zero-reference-eviction.md).

Historical verification before the ownership replacement: all 21 selected
`cargo test -p edgion-gateway --lib shared_data` tests, store-stats serialization,
three LB keepalive tests, workspace check/Clippy, formatting, doc/SSA/metrics/
Pingora/no-tracing/English-only guards passed. One pre-existing DSL integration
Clippy warning remained. Independent review returned LGTM; no full integration
run was claimed. Historical follow-up verification also passed the delayed
module-eviction handle test, 23 cache-residue tests, local runner deadline contract,
and Kubernetes residue-gate shell test. Those results concern the old 60-second
grace and are not fresh verification of owner destruction.

The 2026-09-23 ownership run passed all 226 selected Gateway Wasm tests, including
both module-metric lifetime tests and the SharedData tests. The module-metric
idle-eviction test covers overlapping same-source generations and distinct sources.
The replacement ownership test establishes that owner drop releases unleased
stores, and retained request leases keep their contribution until their last drop.
The existing 75-second residue deadline remains an upper bound; it does not
require introducing a new grace or delaying cancellation.
The same ownership run also passed the live dynamic scenario, static Wasm suite,
and mandatory cache-residue gate with no retained store or metric metadata leaks.

Re-evaluate if recorder installation order, idle eviction, metrics rendering
entry points, store accounting, or module ownership changes. Metrics must be
installed before constructing instrumented stores; a deliberately rejected
series under the cardinality guard remains subject to that guard's policy.

## Resolution references

- `wasm-shared-data-gauge-generation-lifecycle` — pending task created
  2026-09-07; corrected with mutation deltas and destruction symmetry.
- `wasm-shared-data-eviction-grace-exceeds-residue-timeout` — historically, the residue
  deadline was raised above the then-current idle eviction grace, with metric visibility
  and fail-close regression coverage. Running-tick quiescence was closed by
  `wasm-tick-retention-exceeds-residue-deadline` (2026-09-23); its rule lives in
  [tick-quiescence-on-zero-reference-eviction](tick-quiescence-on-zero-reference-eviction.md).
