---
name: etcd-key-content-identity
description: Use when reviewing etcd resource-ingest identity and invalid-document handling; the key is authoritative and the resource controller owns malformed generations.
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
revoked through the existing worker: the event loop removes the authoritative raw object,
records the invalid envelope and enqueues its key. The invalid generation is never ingested.
Initialization writes the diagnostic before the worker starts. At runtime, the event loop records a
pending diagnostic and enqueues the authoritative key; the single per-kind worker performs the status
RPC in queue order. A later valid Apply or Delete clears the pending diagnostic before enqueuing, so
coalescing always converges on the latest event without status-write races.

Federation is not another raw ingest source: it proxies authorized Admin API requests into the normal
ConfCenter writer, whose URL/body identity guards remain authoritative.

## Invalid byte values (ARCH-086)

Key presence and value validity are independent. EtcdWatcher strictly decodes
values from prime, live watch and compaction resync into EtcdDocument::Text or
InvalidUtf8. A recognized key remains present in known-key tracking even if its
value cannot be decoded; resync sends Apply for it, not a synthetic Delete.
Invalid bytes are discarded, never embedded in events, errors or logs.

EtcdResourceController maps InvalidUtf8 to a fixed actionable ParseError using
the same parser boundary as textual YAML failures. Initial failures use the
serial pre-worker status path. Runtime failures remove raw state, retain the
existing RuntimeInvalidEnvelope and enqueue via on_event_key; the single worker
revokes derived state and persists diagnostics inline. Valid repair or actual
Delete clears the envelope. The existing leadership gate, duplicate-error
suppression, InitDone guard, FIFO startup and revision continuity are unchanged.
The unsupported pre-init runtime-Apply policy remains separately documented.

The former watcher dropped initial invalid values, consumed live Put revisions
without delivery, and misclassified invalid present resync keys as deleted.
Likewise, the previous version of this page stated that a runtime identity mismatch retained
the last valid generation was stale: current rejection removes raw state and
revokes it through the worker.

## Regression Coverage and limits

External unit tests under
`edgion-controller/tests/unit/conf_mgr/conf_center/etcd/` cover:

- `watcher_test.rs`: production byte-ingress seams for prime/init dispatch,
  runtime Put/Delete and resync; invalid presence versus actual absence; repair,
  revision updates, known keys and initial valid String ownership transfer.
- `resource_controller_test.rs`: authoritative identity, custom prefixes,
  malformed keys and the production runtime Apply/Delete transitions; actual
  queue/processor revocation and recovery, repeated invalid input, latest-event
  coalescing, fixed diagnostics and ErrorStatus duplicate suppression.

These tests require no etcd server. Initial diagnostic persistence and worker
status RPC wiring are source-reviewed, not live etcd integration evidence.
Clearing an invalid envelope on Delete does not promise removal of an already
persisted error sidecar: the existing worker can return Skipped when both raw
and processed state are absent. That separate limitation remains outside ARCH-086.
Re-evaluate when document carriers, key parsing, acquisition paths, worker or
status ownership, or startup ordering changes.

## Resolution references

- `etcd-ingest-identity-check` — 2026-08-03.
- `ARCH-086` — preserve invalid UTF-8 through the existing ingress lifecycle.
