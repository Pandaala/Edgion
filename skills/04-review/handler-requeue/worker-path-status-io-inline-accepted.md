---
name: worker-path-status-io-inline-accepted
description: Use when reviewing conf_sync worker findings that flag the status read/write IO inside spawn_worker's dequeue loop (Rule-5 "no latency in handler path") as a head-of-line-blocking bug; covers why the IO stays inline and why moving it to an async status-writer / coalescing queue is rejected.
status: accepted-tradeoff
finding_id: workqueue-status-io-inline
closed: 2026-07-18
---

# Status IO Stays Inline in the Worker Path — DESIGN-TRADEOFF

## Conclusion

The conf-center resource controllers do status IO **inside** the single serial
worker (`spawn_worker<K>` in both `file_system/resource_controller.rs` and
`kubernetes/resource_controller.rs`): `read_status_json` before
`process_work_item` (FS mode only), `write_error_status` on parse/identity
failure, and `write_status_value` / `delete_status` / `persist_k8s_status`
after. In K8s mode a status write is a real apiserver API call. This technically
sits under `AGENTS.md` Workqueue-discipline Rule 5 ("no API calls, timers,
sleeps, or other latency in the handler path"), but keeping it inline is a
**deliberate, accepted exception**, not an unfixed bug. The head-of-line latency
is the accepted price of the queue's ordering guarantee.

Fix suggestions of the form "move status writes off the worker into a separate
status-writer task/queue keyed by resource, coalescing repeated writes for the
same key" are **not accepted** — they trade ordering for throughput.

## Core Rationale

**1. The queue's contract is "in order, including durable effects"**

The workqueue guarantees more than per-key event ordering: when item N+1 is
processed, item N's persisted side effects are already in place. The inline
status write is exactly the barrier that makes "in order" mean "in order
*including* durable effects". Moving the write to an async coalescing queue lets
item N+1 start (and read state) before item N's status is durable — the ordering
violation the user rejected ("a slow write must not break the in-order principle, or problems follow").

**2. FS mode is a genuine read-your-writes loop**

In FileSystem mode the worker is the *sole serial writer and reader* of the
`.status` file: it reads `read_status_json` (`resource_controller.rs` ~line 623)
to compute `status_changed`, then writes the file back. If the write were
deferred to an async queue, a later event for the same key could read the old,
not-yet-flushed status → wrong `status_changed` decision. Patching that with an
in-memory status cache would then leave the *persisted* status trailing the
in-memory truth, i.e. behind the config conf_sync has already shipped to the
data plane (worse on crash/restart).

**3. The cost is self-limited, and the loop is otherwise CPU-bound**

The write is gated on `status_changed`. A steady-state relist/resync of a large
resource set (even ~100k) where most statuses are unchanged does **zero** writes,
so the loop is pure CPU (deserialize → validate/compile → build index /
serialize), which serial processing handles fine. The IO only bites in the
narrow window where many resources genuinely change status at once (a true bulk
create/delete). Parallelizing the queue would not even help that window: N
workers would all block on the same apiserver, so throughput would still be
bounded by apiserver write latency, not worker count.

## Fix Suggestions Not Accepted

- "Move status writes to an async status-writer task / coalescing queue keyed by
  resource" — breaks the ordering / read-your-writes invariant (rationale 1–2);
  last-write-wins coalescing fixes same-key write order but not the read side of
  a subsequent same-key or cross-key item.
- "Shard the per-kind queue into multiple workers by namespace hash" — separate,
  larger re-architecture; namespace is the wrong shard key because the hottest
  kinds' consistency domain is the target Gateway (cross-namespace via
  ReferenceGrant), so it would add concurrent writers to shared per-kind
  registries. If ever pursued it is a `superpowers:brainstorming` design task,
  not a status-IO fix.
- "Cache `read_status_json` alongside the resource in the processor cache (FS
  mode) so the common path does no read IO" — does not violate ordering, but the
  read is cheap local file IO, is absent in K8s mode (passes `None`), and the
  write (the part the original todo called the real cost) is untouched;
  insufficient value to justify the change. Re-open only if measurement shows the
  FS read dominates.

## Re-evaluation Triggers

Re-open this decision only if:

- A `superpowers:brainstorming` design task adopts an ordering-preserving scheme
  (e.g. a durability barrier that lets item N+1 start only after item N's status
  is persisted, or a per-key in-memory status cache paired with a guaranteed
  flush-before-dependent-read) that is proven not to break rationale 1–2.
- Measurement under a bulk create/delete (timing `process_work_item` with and
  without the status IO, as the original todo required) shows the inline status
  IO — not the CPU reconcile — is the dominant convergence cost, AND an
  ordering-safe remedy exists.

## Reference Cases

- `tasks/todo/worker-path-blocking-status-io.md` (filed 2026-07-17, closed
  2026-07-18 as accepted tradeoff) — the todo this decision resolves.
- `AGENTS.md` "Workqueue discipline (controller conf_sync)" — Rule 5 and the
  "Deliberate, accepted exception to Rule 5" note.
- `docs/superpowers/specs/2026-07-17-conf-sync-event-ordering-design.md` — the
  event-ordering design that named the violation; note this IO is *serial* and
  is not the parallel-task reordering bug that design fixed.
- `edgion-controller/src/conf_mgr/conf_center/file_system/resource_controller.rs`
  (`spawn_worker`, `read_status_json` ~623, `write_status_value` ~645).
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs`
  (`persist_k8s_status`, worker ~642-660).
- Sibling: `handler-requeue/status-write-no-retry-accepted.md` (controller-01) —
  the *retry* tradeoff on the same status-write path.
