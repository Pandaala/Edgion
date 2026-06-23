---
name: es-bulk-ingest-empty-endpoints-fail-closed
description: Use when reviewing LinkSys Elasticsearch bulk_ingest_loop empty-endpoints panic / out-of-bounds index findings — EsLinkClient::from_config now bails before building the client, so the empty Vec never reaches the spawned loop.
---

# LinkSys ES bulk_ingest_loop Empty endpoints Panic

**Status / verdict**: fixed (link-sys-FB01, 2026-05-25)

When reviewing LinkSys ES findings of the form "`bulk_ingest_loop` panics on an empty `endpoints` list / `endpoints[endpoint_idx % endpoints.len().max(1)]` indexes `endpoints[0]` out-of-bounds when endpoints is empty / a CRD with `endpoints: []` registers then silently kills the bulk task on first flush (link-sys-FB01)": **fixed (link-sys-FB01, 2026-05-25)**. `EsLinkClient::from_config` (`edgion-gateway/src/link_sys/providers/elasticsearch/client.rs`) now bails with `"Elasticsearch endpoints list is empty for '{}'"` before building the client, mirroring `EtcdLinkClient::from_config`. The fix is at the origin, not the symptom: `register_for_full_set` (`runtime/store.rs:535`) logs the `from_config` Err as `"from_config failed"` and never inserts/`init`s the client, so the empty Vec can never reach the spawned loop and the four `bulk.rs` slice-index sites are unreachable for the empty case (their `.max(1)` divide-by-zero guards stay as harmless now-dead belt-and-suspenders). `#[schemars(min_items = 1)]` was intentionally NOT added — no `edgion-resources/src/resources/link_sys/` type uses CRD-layer length annotations and Etcd guards only in `from_config`, so the guard stays consistent. Unit regression `test_empty_endpoints_returns_error` (mirrors the Etcd test). Re-reports of the empty-endpoints panic form are not accepted; verify `from_config` bails on `endpoints.is_empty()` before re-reporting.
