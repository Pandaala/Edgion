# ctrl-proc-04: Bounded workqueue channels make a worker's own requeue block, so a large kind can deadlock itself

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:180-188` (capacity 5000)
  - `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:337-341` (both channels bounded)
  - `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:395-431,488` (delay loop awaits `ready_tx.send`)
  - `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:534,586` (`enqueue`/`enqueue_after` await)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:391,1020-1027`
  - Self-requeue sites: `handlers/http_route.rs:221,323,435,809`, `handlers/grpc_route.rs:60,215,308`, `handlers/gateway.rs:694,742,798,834`, `handlers/backend_tls_policy.rs:389-452`, `handlers/edgion_backend_traffic_policy.rs:534-572`

## Summary
Each per-kind `Workqueue` uses two bounded `mpsc` channels of 5000 entries: ready and delay. Both
`enqueue` and `enqueue_after` `await` on `send`. The single delay-loop task also awaits
`ready_tx.send` while promoting due items, and does not drain `delay_rx` during that wait. A worker that
requeues keys of its **own** kind (HTTPRoute/GRPCRoute delegation, Gateway peers, policy siblings) can
therefore block on `delay_tx.send` while its own ready channel is full. Only that same worker drains the
ready channel, so the kind deadlocks permanently. This violates workqueue Rule 2 ("dequeue processing … must
not wait") and Rule 5, and the bound provides no protection that dedup does not already provide.

## Current behavior (evidence)
1. `Workqueue::new` creates `mpsc::channel(config.capacity)` twice (`workqueue.rs:339-340`).
   `ResourceProcessor::new` always uses `Workqueue::with_defaults` (`processor.rs:391`), so the capacity of
   5000 cannot be configured.
2. `requeue_with_chain` → `enqueue_after` → `self.delay_tx.send(delayed).await` (`workqueue.rs:586`).
3. The delay loop pops due items and calls `promote_delayed_item(..).await`, which does
   `ready_tx.send(key).await` (`workqueue.rs:488`). While it waits, the `select!` is not polling
   `delay_rx`, so the delay channel fills up behind it.
4. Deadlock sequence for HTTPRoute with more than about 10,000 distinct pending keys (reachable through a
   listener change on a large Gateway, `after_publish` → requeue all routes, or the per-Gateway ownership
   replay in ctrl-proc-03):
   - The ready channel holds 5000 keys. The delay loop blocks promoting key 5001.
   - The delay channel fills with 5000 more scheduled keys.
   - The HTTPRoute worker dequeues a delegating parent, and `parse` calls `_ctx.requeue("HTTPRoute", child)`
     (`http_route.rs:323`). That blocks on the full `delay_tx`.
   - The only consumer of the HTTPRoute ready channel is now blocked, so the delay loop never unblocks.
     The Gateway worker that issued the bulk requeue and the K8s watcher's `on_event_key` (which awaits
     `enqueue`) also block behind it.
5. Dedup (`pending` + `scheduled`) already bounds both queues to at most the number of distinct keys of the
   kind (`skills/04-review/cpu-memory/review-evidence/workqueue-delay-heap-no-size-cap.md`). The channel
   capacity therefore adds no memory bound. It only adds the blocking behavior.
6. `skills/04-review/controller/regression-guards/liveness-probe-and-lifecycle-supervision.md` records as
   an accepted *detection* limitation that "a bounded-workqueue send from inside a consumer … wedges silently
   with probes green" after readiness. `skills/02-features/02-config/00-controller-config.md` asks
   operators to raise `cache_ready_deadline_secs` when a kind approaches 5000 objects. The hazard is known
   operationally, but the root cause has not been addressed.

## Industry comparison
client-go / controller-runtime `workqueue` is an unbounded FIFO deduplicated by a `dirty` set, plus a
`processing` set and a delaying heap fed by an unbounded channel (`waitingForAddCh` with a large buffer, and
`AddAfter` never blocks the caller in practice). `Add` never blocks a reconciler, so a reconciler can
requeue its own kind safely. Backpressure, where needed, is applied at the informer and not inside
reconcile.

## Impact
At scale (thousands of HTTPRoutes using delegation, or large policy sets with sibling requeues), the kind's
reconciliation stops permanently with no error. Liveness probes stay green after readiness, and the
Controller serves stale configuration until it is restarted.

## Suggested direction
Make every enqueue path non-blocking, since dedup already bounds memory:
- Replace the ready channel with an unbounded channel, or with a `Mutex<VecDeque>` + `Notify`, gated by
  `pending`. Replace the delay channel with an unbounded channel, or push directly into a
  `Mutex<BinaryHeap>` shared with the delay task + `Notify`.
- Keep the high-water warning as an observability signal, based on `pending.len()` relative to the store
  size instead of a fixed capacity.
- Remove the "approaches 5000" sizing rule from the config docs once the bound is gone.

## Notes
- Not covered by `workqueue-delay-heap-no-size-cap.md`. That page argues the heap cannot grow unboundedly,
  which supports this finding: the channel capacity is redundant. It does not consider self-requeue
  blocking.
- `liveness-probe-and-lifecycle-supervision.md` accepts that such stalls go *undetected*. It does not
  accept that the stall should exist. This finding removes the stall instead of adding detection.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:181-188,339-340`: `capacity: 5000`, two `mpsc::channel(config.capacity)`.
- `workqueue.rs:406-412,488`: delay loop awaits `promote_delayed_item` → `ready_tx.send(..).await` inside the sleep arm, not polling `delay_rx` meanwhile.
- `workqueue.rs:534` (`self.tx.send(key).await`) and `:586` (`self.delay_tx.send(delayed).await`): caller blocks when full.
- `skills/04-review/controller/regression-guards/liveness-probe-and-lifecycle-supervision.md` accepts only that such wedges go undetected; it does not accept the wedge. Trigger needs >~10k distinct pending keys of one kind, so P2 (not P1) is right.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs` unchanged since `feature-08-30`: capacity 5000 (`:180-188`), two bounded channels (`:339-340`), awaited `ready_tx.send` in the delay loop (`:488`) and awaited `enqueue`/`enqueue_after` sends (`:534,586`).
- Self-requeue handler sites unchanged except mechanical chrono→jiff type edits in BTP/EBTP handlers (line counts preserved).
