---
id: ARCH-087
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-425
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

Title: Create the cache watch notification future before inspecting the event store

The typed cache watcher can miss the last notification for a newly committed event because it creates its Notified future after checking that the event store has no new data. The event remains available in the ring, but the watcher can sleep until another event or lifecycle transition supplies a later wakeup.

ServerCache::start_watcher_task in edgion-controller/src/conf_sync/cache_server/cache.rs:103-107 reads get_events_from_sync_version(from_version) under the store read lock and then releases the lock. When the result contains no newer event, lines140-155 construct a new notify.notified() future inside the waiting select. The producer at247-253 applies its event under the write lock, releases that lock and calls notify_waiters(). The Controller binary explicitly runs a multithreaded Tokio runtime, and the per-kind worker and per-client watch task are separate tasks; serial resource processing does not serialize the watcher with the producer.

A supported interleaving is: the watcher checks versionv and finds itself current; a runtime update or delete commits versionv+1 and broadcasts before the watcher constructs its waiting future; the watcher then constructs the future and waits. The store containsv+1, but this task does not check it again. If there is no later event for that kind, no store-generation replacement and no receiver close, this watch remains atv. This is a source-supported convergence failure, not a reproduced deployment incident or a claim that every update is lost.

The pinned Tokio1.53.1 implementation establishes the boundary. Notify::notified() captures the current notify_waiters call counter when constructing the future. Its documentation guarantees broadcast delivery to an already-created future even if that future has not been polled. notify_waiters() with no waiting task increments the broadcast counter without creating a pending notify_one permit. A future created after that broadcast captures the updated counter and has no older broadcast to consume. The store lock correctly orders data mutations but does not bridge the later future-creation gap.

The real downstream pipeline does not recover this same-generation event on a timer. ServerCache::watch_json and ConfigSyncServer::watch await their upstream receivers. The gRPC watch wrapper's one-second check detects a changed server ID, not a newer resource version on the unchanged server. Gateway event_dispatch awaits stream messages or channel-generation cancellation; it relists on received errors, stream termination or a generation change. With a healthy idle connection and unchanged generation, none of those branches notices the ring event that the typed producer stopped checking.

Create a fresh Notified future at the start of each typed watcher iteration, before reading EventStore, and await that same future in the no-events select. With the current broadcast API, Tokio's construction-time guarantee closes the gap without a polling loop or an explicit registration framework. An update before construction is visible to the subsequent store read; an update after construction either appears in that read or completes the already-created future. Do not replace broadcast with notify_one, which would change the all-watcher contract.

Preserve version allocation under the write lock, per-key stale-event protection, bounded ring capacity, TooOldVersion/VersionUnexpect recovery, normal receiver-closure cleanup, per-client send backpressure and the existing reload polling backstop. No send timeout, stream-age limit, cross-kind epoch, new registry, metric or logging family is required. This is distinct from ARCH084: it concerns a live watcher missing a producer notification, not a stopped federation session failing to cancel descendants.

The full existing cache and store test bodies cover event application, concurrent version allocation, stale-key isolation, ring wrap and version errors. They do not exercise the store-check-to-notification-future interleaving. Add a deterministic regression around the actual scan/wait boundary: commit and broadcast after a no-events observation but before waiting, then establish delivery of that single event without publishing another event. Preserve receiver-closed exit and multi-watcher broadcast coverage. No tests, builds or applications were run in this read-only review.
