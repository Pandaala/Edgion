---
id: ARCH-119
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-426
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Keep metadata observation tied to the last delivered count

## Current ownership and concrete trigger

ConfigSyncGrpcServer::watch_server_meta creates its initial ServerMetaEvent with registry.count() at grpc_server.rs:809–814, awaits that send, then initializes last_count with a second registry.count() at822. Later it reads the count only after ClientRegistry::wait_for_change returns. The one-second tick at865 checks server identity, not same-server instance count.

A supported interleaving has two registered gateway IDs when the initial event is constructed. Another gateway registers and broadcasts before this stream initializes last_count. The initial queued event reports two, but last_count is initialized to three. The broadcast has no durable count-event payload, and the next wait_for_change future is created after that broadcast. With no further membership change or server replacement, the consumer retains two while this stream regards three as already observed. On the real multithread runtime another task may register between these operations; the initially empty send's readiness does not serialize unrelated workers.

There is also a steady-state gap after a successful count send or a periodic tick: each loop constructs its next notification future only inside the select, with no corresponding current-count read before parking. A broadcast between iterations need not wake that later-created future. The count is current in the registry, but not projected onto every metadata stream.

Actual Gateway run_leader_session sets gateway_instance_count from delivered events and has a first-message deadline only. RateLimitLocalConfig derives Cluster scope's best-effort allowance from that count. This finding establishes a missed same-server membership projection, not an exact global quota promise or deployed incident.

## Bounded improvement and alternative

Use the count actually put in the initial event as the stream's emitted baseline: the snapshot successfully enqueued into the existing metadata channel, without implying a remote acknowledgement. At the start of each subsequent loop, construct the actual Tokio Notified before inspecting the current registry count. Provide a narrow ordinary ClientRegistry method returning the Notified created by its private Notify, or an equivalent registry-owned scan/wait helper with this construction ordering. Merely constructing the existing async wait_for_change wrapper before the count read is insufficient: its body creates Notified only when first polled. If the scanned count differs from the emitted baseline, send that snapshot under existing bounded backpressure and advance the baseline to that same snapshot after a successful send; otherwise await the already-created Notified alongside receiver/lifecycle/server-switch checks. Repeat the observation on the next iteration so changes during a send are reconciled. Preserve notify_waiters broadcasting and the latest-state projection contract; no delivery of every transient membership state is required. Keep one metadata observation owner and existing channels rather than adding another count cache or timer.

A simpler alternative is to read and reconcile membership on the existing one-second server-switch tick, with an explicit bounded-convergence contract. That avoids indefinite staleness but leaves notification ordering dependent on the tick; evaluate this small change against the pre-created scan/wait approach. Initial baseline must still mean the last event emitted, not an arbitrary later unsent observation.

Preserve minimum-one counting, unique-client identity, metadata queue capacity eight, event-driven count projection, current timestamp/server IDs, provider None during reload, terminal server-switch event, process/term cancellation and existing cold binary logging. No extra request-path work, new metrics, generic watch framework, forced relists or send timeout is needed. Registration lifetime itself is ARCH-118 and must remain separate.

## Evidence, prior decisions and verification

ClientRegistry::register/unregister at client_registry.rs:28–54 mutate the map then notify_waiters; wait_for_change at64–65 constructs Notified when polled. ARCH-087 independently confirms pinned Tokio1.53.1 construction-time notify_waiters semantics for the typed resource watcher. That finding's primary is cache_server/cache.rs and its remedy cannot repair this metadata RPC's unsent bootstrap baseline. This report applies the same original notification semantics to a different projection owner, with an additional independently concrete initial-event/last_count mismatch; it introduces no duplicate synchronization implementation.

The original Controller-side link architecture promises count-change projection onto WatchServerMeta; bounded-payload-compression distinguishes its eight-entry metadata queue from resource Watch's one-entry chain. Accepted send-await backpressure governs deliberate slow-reader retention, not a healthy stream marking an unsent count as delivered. Transactional channel selection preserves the sole process metadata coordinator and does not repair same-server observations. Existing full grpc_server_test.rs covers reload/shutdown/term and sequential real TLS updates, but not an interleaved initial count or a last membership broadcast between iterations. ClientRegistry tests verify a delayed wakeup, not projection continuity.

Future tests should force one registration between initial event construction and subsequent observation, then assert delivery of the new count without another membership change. Exercise a broadcast after the actual Notified is constructed and after a no-change scan, proving that the same future completes without another mutation; pre-creating only the async wrapper must not be treated as registration. Cover a change during a bounded send, multiple metadata consumers and unregister transitions. Preserve receiver cleanup, unique-ID counting and current server/term switch tests. No tests or applications were run. Source-only evidence establishes feasibility, not timing measurements or a live incident.
