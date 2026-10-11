---
id: ARCH-084
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-428
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

Title: Cancel session-owned federation watch tasks when the connection future is dropped

A normal supervised federation stop can finish the client task while leaving its idle watch forwarding task and underlying cache-watch pipeline alive. Session cancellation currently depends on reaching the tail of `connect_and_run`; cancellation of that future skips the tail.

`fed_client::run` in `edgion-controller/src/fed_sync/fed_client/mod.rs:231–258` selects between `shutdown.changed()` and `connect_and_run`. If shutdown wins while a session is running, it returns and drops the connection future. `connect_and_run:448–454` cancels the active watch and stats tokens only after its inbound loop finishes normally. Dropping the future does not execute those statements. The pinned `tokio-util 0.7.19` implementation confirms that `CancellationToken::Drop` merely decreases its handle reference count; it does not cancel cloned task tokens. Its existing `drop_guard` API expressly provides cancellation on drop.

A successful WatchRequest spawns a forwarding task at lines 807–872. That task owns an uncancelled token clone, the event receiver, an outbound sender and a ConfMgr clone. Its select waits for token cancellation, a resource event, or a one-second CSS-generation check. It does not wait for the outbound sender's receiver to close. If the Controller's CSS generation remains unchanged and that resource has no further event, closing the federation session does not end this forwarding task: it continues to retain the receiver and poll the generation even after the supervised client JoinHandle has returned.

The underlying producer lifetime is concrete. `ConfigSyncServer::watch` converts a registered watch object's receiver and stops on its downstream sender closing. `ServerCache::watch_json` similarly retains the typed receiver until its downstream receiver closes. `ServerCache::start_watcher_task` retains the EventStore/notification owner and waits for either a store notification or receiver closure. The federation forwarding task's retained receiver keeps those cleanup signals from propagating through the pipeline. A later resource event can make the failed outbound send terminate the task, and a later CSS replacement can also end it; the witness is a quiet, unchanged store, not an unconditional forever leak.

Give the already existing session cancellation tokens ownership that cancels them when the connection future is dropped, using the available cancellation-token drop-guard mechanism or an equally small private owner. Register that ownership before spawning the session tasks. Keep replacement-watch cancellation local to the active watch, and preserve the existing explicit normal-tail cleanup. Ensure a session task waiting inside an outbound send can also observe cancellation; a cancellation branch only at the outer select cannot interrupt an await inside an already selected branch. This needs no general session framework, new monitoring family, registry scan, or mandatory forced await of all spawned tasks.

Preserve authorization snapshot semantics for a running watch, event/version bounds, watch request IDs, the one-active-watch replacement rule, stats generation checks, error ordering and normal disconnect behavior. The documented ten-second supervisor timeout intentionally permits a hung top-level client to detach; that is a separate accepted failure mode. This finding applies to an ordinary cooperative stop whose top-level task has already exited, and does not claim post-stop transmission or synchronous destruction of every descendant task.

Existing client tests cover stats collection modes and denied-watch admission. Supervisor tests prove that the fake top-level client observes shutdown and returns, but do not establish descendant watch receiver/token cleanup. A focused lifecycle regression should arrange a quiet accepted watch, cancel the connection future through the real shutdown boundary, and observe that the forwarded receiver is eventually dropped without needing a resource event or CSS replacement. Cover a blocked bounded-channel send as well. The original accepted orphan-task guidance concerns process-lifetime bootstrap caches; it explicitly does not justify a shutdown-capable reconnect/session task retaining old session owners. No tests or applications were run in this read-only review.
