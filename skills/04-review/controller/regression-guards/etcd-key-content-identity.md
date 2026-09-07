---
name: etcd-key-content-identity
description: Use when reviewing etcd resource-ingest identity findings; the etcd key is authoritative and mismatched metadata identity is rejected before the raw store or resource processing.
status: fixed
finding_id: etcd-ingest-identity-check
closed: 2026-08-03
knowledge_type: regression-guards
---

# Etcd resource keys are authoritative over content identity — FIXED

## Conclusion

`EtcdResourceController` now strictly parses the final `/conf/{kind}/{namespace}/{name}` key shape
and compares every parsed resource's `key_name()` with that identity before initialization or runtime
processing. A mismatch is rejected with an `IdentityMismatch` status under the etcd-key identity; it
never reaches `LocalRawStore` or `ResourceProcessor`. Malformed keys, empty names, and extra path
segments are rejected before ingest. The ResourceController receives the exact configured
`conf_prefix`, strips it first, and then requires exactly `kind/namespace/name`; this both permits a
custom `key_prefix` containing `/conf/` and rejects a nested backend marker after the real config root.

This applies to initial `InitApply`, an `Apply` received before `InitDone`, ordinary runtime watch
events, and compaction resync (which uses the same runtime `Apply` event). Re-reports that cover only
the startup list or only the live watch are incomplete.

## Rationale

The etcd key (`{prefix}/conf/{kind}/{namespace}/{name}`) is the authoritative storage location.
Apply and delete must derive the same resource identity from it. Previously apply used
`metadata.namespace` / `metadata.name`, while delete used the etcd key, so a hand-written value could
overwrite another content-keyed resource and then survive deletion of its own etcd slot.

The controller rejects mismatches instead of backfilling metadata from the key. Silent backfill would
hide invalid stored content and make the value observed through direct etcd inspection disagree with
the resource synchronized by Edgion. During a runtime mismatch, the last valid processed generation is
left in place, matching FileSystem-mode rejection semantics; the invalid generation is never ingested.
Initialization writes the diagnostic before the worker starts. At runtime, the event loop records a
pending diagnostic and enqueues the authoritative key; the single per-kind worker performs the status
RPC in queue order. A later valid Apply or Delete clears the pending diagnostic before enqueuing, so
coalescing always converges on the latest event without status-write races.

Federation is not another raw ingest source: it proxies authorized Admin API requests into the normal
ConfCenter writer, whose URL/body identity guards remain authoritative.

## Regression Coverage

Unit tests in `edgion-controller/src/conf_mgr/conf_center/etcd/resource_controller.rs` cover
namespaced match/mismatch, cluster-scoped identity, empty and malformed names, extra path segments, a
custom prefix containing `/conf/`, a nested repeated backend marker, and pending-diagnostic clear/take
ordering.

## Resolution references

- `etcd-ingest-identity-check` — 2026-08-03.
