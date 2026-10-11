---
id: ARCH-086
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-396
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-086: Preserve invalid UTF-8 documents through the etcd ingress lifecycle

## Problem and current ownership

EtcdWatcher discards a present configuration key when its value is not UTF-8. That bypasses the existing resource-controller owner of malformed generations and gives initial list, live watch, and compaction resync different meanings for the same invalid stored document.

In edgion-controller/src/conf_mgr/conf_center/etcd/watcher.rs:319-344, runtime processing first assigns revision = kv.mod_revision(). For a recognized, subscribed Put, UTF-8 decoding failure then logs and continues without delivering an Apply or Delete. Consider a valid /edgion/conf/HTTPRoute/default/r1 followed by a Put containing invalid UTF-8 at that same key. The raw store and processed generation receive no transition and remain unchanged; a reconnect resumes after the consumed revision. No repair event or later relist is guaranteed to occur.

This differs from an invalid UTF-8-free YAML document delivered through Apply. EtcdResourceController removes the authoritative raw object, records RuntimeInvalidEnvelope with ParseError, and enqueues the authoritative key (resource_controller.rs:274-291). Its existing single worker revokes derived state through process_work_item(..., None, ...) and writes the error status inline (resource_controller.rs:421-440; resource_processor/processor.rs:551-555,912-939). Invalid input is therefore an owned generation transition, rather than permission to retain the previous generation silently.

The same watcher gate also filters invalid byte values from prime (watcher.rs:94-104) and resync_phase (watcher.rs:186-198). Initial loading emits no invalid-document diagnostic. During resync, a previously known key still present with an invalid value is absent from current_by_kind, so it is emitted as Delete (watcher.rs:219-233), clearing the old generation without preserving a malformed-document diagnostic. Decode success is incorrectly serving as key-existence evidence.

## Evidence and reachability

The actual native SDK exposes KeyValue::value() as &[u8]; value_str() separately performs fallible UTF-8 decoding. Edgion's get_prefix_with_revision preserves those values as Vec<u8> (edgion-common/src/link_sys_core/providers/etcd/ops.rs:133-157). This is an actual byte-valued etcd ingress boundary even though Edgion's normal typed CRUD writer emits textual YAML. The witness requires an invalid external/configuration-store write, not a pre-init Apply injected into an unsupported lifecycle.

EtcdController::run starts runtime watch only after spawn_all_controllers completes phased initialization (controller.rs:130-146). The original non-kubernetes-pre-init-runtime-events-unsupported.md explicitly distinguishes unsupported pre-init runtime injection from supported malformed initial documents and post-init updates, whose raw removal, worker revocation, and status behavior remain required.

## Bounded improvement

Preserve the distinction between a present decoded document and a present document that failed decoding in the existing watcher-to-resource-controller pipeline. A small owned decode outcome in the existing initial/runtime event payloads, or equivalent explicit invalid-document events for those phases, can carry the authoritative key and a fixed non-sensitive failure description without retaining or logging the invalid bytes. The exact type should be selected against the current event consumers rather than introducing a generic ingress framework.

For supported runtime invalid input, validate/extract identity from the configured etcd key, remove the raw object, store the existing invalid envelope, and enqueue through on_event_key. Let the existing worker perform derived-state revocation and inline error-status IO. For initial invalid input, use the existing serial pre-worker diagnostic path. Prime and resync must track recognized present keys independently of successful decoding so an invalid present value is not misclassified as deletion. Later valid Apply or real Delete must clear the invalid envelope through the existing transitions.

The value is one consistent ownership boundary for malformed documents across three production acquisition paths. This is a bounded Controller watcher/event/consumer change; no new workqueue, background status writer, Gateway policy change, or SDK-wide transport redesign is needed.

## Alternatives and preserved invariants

Skipping the value preserves the current silent stale generation. Emitting Delete alone revokes state but loses the distinction between deletion and a present invalid document, reproducing the current resync mismatch. Lossy UTF-8 replacement or fabricated malformed YAML would conceal the actual decoding outcome and should not become the resource parser's input contract.

Preserve exact configured-prefix and resource-key validation, canonical kind mapping, phased init ordering, idempotent InitDone, one workqueue and one serial worker per kind, runtime Apply/Delete coalescing, current compaction/revision continuity, bounded channel backpressure, leadership-gated status persistence, and repeated-error deduplication. No raw document bytes belong in diagnostics or observability. Preserve existing logging ownership and restraint; this is not a request for additional traffic logs or metrics.

## Existing tests, original decisions, and limits

The complete watcher tests cover phased document transfer and releasing retained documents while preserving revision/known keys. They do not cover byte-decoding outcomes. etcd_valid_to_malformed_to_repair_revokes_and_recovers covers the existing invalid-envelope revocation/repair contract by directly modeling the resource transition; it does not exercise watcher delivery. Focused regression coverage should distinguish valid-to-invalid-byte runtime transition, invalid initial document, invalid-but-present resync versus actual deletion, and subsequent repair, while retaining current transfer and initialization tests.

The accepted inline-status-IO decision and single-worker initialization guard remain intact. The authoritative-key review page's statement that runtime identity mismatch retains the last valid generation is stale relative to current source; this finding instead relies on the current malformed-document implementation/test and the original supported-lifecycle decision. Its separate statement that initial, watch, and resync must share authoritative identity handling remains relevant. The older config-center overview's no-status-persistence table is also not evidence against the current status owner.

This is a static source-supported witness, not a reproduced etcd incident or test execution. There is no claim that ordinary typed Admin writes can emit invalid UTF-8, that every invalid key is ingestible, or that transient status-write failures are eliminated. No new payload-size, retry, election, or whole-response policy is proposed.
