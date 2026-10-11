---
id: ARCH-093
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-397
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-093: Deliver the complete FS initialization before publishing readiness

## Problem and current ownership

FileSystemWatcher uses a fixed-capacity broadcast ring for a production pipeline with one resource-controller receiver per kind. Initialization replay does not await delivery or apply backpressure. A sufficiently large static configuration can therefore lose initial documents while the retained InitDone still permits that kind to publish readiness.

file_watcher.rs:81-99 creates broadcast::channel(4096). dispatch_init:221-251 removes the subscribed kind's snapshot, then synchronously sends Init, every InitApply and InitDone. There is no suspension point between those sends. The snapshot is consumed, so overwritten documents have no retained replay source.

A concrete witness is4096 distinct, correctly named, valid HTTPRoute files under the canonical namespace directory. Prime accepts this count; there is no document-count admission limit. If the receiver has not drained the channel during replay, Init plus4096 documents plus InitDone produce4098 messages in a4096-slot ring. Init and the first document are overwritten. This interleaving is possible when the newly launched receiver has not run yet; a single-threaded test can establish it without a scheduler-timing assumption.

The actual pinned Tokio1.53.1 implementation rounds capacity up to a power of two, so4096 remains4096. send overwrites the oldest occupied slot and still succeeds while a receiver exists. recv reports Lagged, advances to the oldest retained message and resumes delivery. FileSystemResourceController:505-520 only warns on Lagged; it neither reloads nor fails initialization. Its retained InitDone branch:277-316 marks the cache ready and starts the sole worker without proving that all initial documents arrived.

## Why existing recovery does not repair this witness

The common spawn path registers and launches one receiver per kind before phase dispatch. Its readiness gates observe the processor-ready state, rather than an expected initial-document count. Losing InitApply is therefore not detected by those gates.

The worker's disk reread repairs a key that is actually enqueued. A lost initial document has no corresponding raw object or queued key merely because PathRegistry recorded its path. Runtime PollWatcher establishes its baseline after initialization; an unchanged file already present in that baseline does not independently produce a repair Apply. No later modification or manual reload is guaranteed.

This concerns supported initial documents and reload initialization, not arbitrary pre-init runtime Apply injection. It requires no high-churn workload, malformed bytes, duplicate paths or unsupported API version.

## Bounded improvement

Replace the FS per-kind broadcast transport with the existing single-consumer bounded mpsc pattern already used by EtcdWatcher. Keep the current FileSystemEvent payloads, phase ownership and LocalControllerBackend associated receiver boundary; do not introduce a generic watcher framework or a second processing queue.

Move each initial event into an awaited send, after releasing sender-map and primed-snapshot guards. Maintain FIFO Init -> InitApply* -> InitDone delivery. Treat receiver closure during initialization as a bounded startup failure through the existing error path, using metadata and a fixed error description rather than formatting resource payloads. Preserve shutdown handling when introducing awaited runtime sends.

The change is local to FS watcher aliases/subscription/sends, the FS resource-controller receive match and affected channel-wiring tests. Production has one subscriber per kind; current FS tests do not require multiple simultaneous subscribers to the same kind. The actual Etcd transport provides a concrete implementation precedent without requiring shared public abstractions.

Awaited sends impose backpressure when a consumer is slow. The centralized FS dispatcher may then wait behind that kind; this is a real tradeoff to assess during implementation. It preserves documents instead of silently publishing a partial initial cache. It does not add parallel handlers, an asynchronous status writer, automatic full relists, delivery acknowledgements, metrics or payload copies.

## Alternatives and preserved contracts

Increasing the ring capacity merely moves the failure threshold. Adding scheduler yields does not guarantee that a receiver drains enough messages. A broadcast replay/acknowledgement layer adds ownership and recovery state despite the existing single-consumer model.

Preserve deterministic collision winners, atomic PathRegistry replacement, canonical absolute/nested paths, pending API-delete behavior, phase-specific snapshot consumption, empty-kind completion and runtime release of leftover documents. Preserve provider/Gateway readiness gates, runtime start after phased initial dispatch, idempotent InitDone, one workqueue and one serial worker per kind, key-only runtime apply/delete enqueueing, delete/recreate coalescing, malformed-generation revocation and inline status IO.

This proposal does not eliminate the accepted prime-to-PollWatcher-baseline gap or change PollWatcher selection, content comparison or interval heuristics. Bounded mpsc limits retained event count; it does not establish a process-wide byte bound or bound the existing raw callback channel.

## Original decisions, tests and limits

The FS architecture accepts the scan-to-runtime-baseline gap and snapshot coalescing. The resource-controller comment accepts possible runtime lag for low-churn FS use. Neither establishes that dropping documents from a large static initial scan and then publishing ready is required behavior.

The original conf_sync backpressure and fixed EventStore ring decisions concern different consumers. Those rings detect TooOldVersion and force an authoritative relist. The FS broadcast receiver has only a warning and no corresponding recovery. This finding does not reopen those accepted transport policies.

The full existing watcher tests cover small phased replay, late subscription, empty kinds, document release, collision selection and replay by a second watcher generation. The actual resource-controller duplicate-completion test protects one worker; delete/recreate and malformed/identity repair tests protect worker transitions. None establishes complete initial delivery above the channel capacity.

Future verification should drive the real private phase-dispatch boundary with more documents than its channel capacity and a concurrently draining consumer, assert every document and exactly one Init/InitDone in order, and cover consumer closure and preserved shutdown. Retain existing phase, reload, worker and malformed-generation tests. Use in-process tasks/channels and temporary files where needed; no network topology is necessary.

This is a static source-supported interleaving, not an executed reproduction, benchmark or production incident. No throughput, exact retained-memory or universal startup-deadline claim is made. Existing lexical kind selection belongs to ARCH047 and is not re-reported here.
