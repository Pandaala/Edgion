# D0076 — Wasm Shared-data Gauge Generation Identity and Lifecycle

## Meta
- Status: needs-design
- Severity: medium
- Discovered in: B0114 Wasm shared-data analysis
- Category: observability-lifecycle
- Canonical task: [`tasks/todo/wasm-shared-data-gauge-generation-lifecycle.md`](../../../todo/wasm-shared-data-gauge-generation-lifecycle.md)

## Evidence
Each live `ModuleEntry` owns an independent shared-data store, while the gauge uses an operator-facing source label. Overlapping configuration variants, reload or OCI refresh generations, and in-flight evicted entries can therefore overwrite one another's value. The current store drop path also does not return the last live series to zero.

## Why deferred
A correct fix must aggregate all simultaneously live stores with the same bounded label and retire the series only after the last store drops. It crosses store, ModuleEntry, source-label, registry, and inventory ownership; adding generation labels would instead create unbounded stale series. B0114 only repairs CAS and capacity linearization inside one store.

## Integration-test impact
Deterministic inline lifecycle tests can cover interleaved stores, overlapping generations, arbitrary drop order, registration/drop races, and final zero. No existing integration-test edit or new integration issue is needed.
