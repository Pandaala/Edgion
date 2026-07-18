# `on_delete` bypasses the workqueue while `on_apply` enqueues

**Status**: done (2026-07-17)
**Priority**: P2 — architectural; two tasks can process one kind in parallel
**Filed**: 2026-07-17, found while diagnosing the conf_sync cache-residue incident

## The asymmetry

`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:371-380`:

```rust
pub async fn on_apply(&self, obj: &K) {
    let key = make_resource_key(obj);
    self.workqueue.enqueue(key).await;      // enqueue -> single worker -> serial
}

pub async fn on_delete(&self, obj: &K) {
    let ctx = self.create_context(TriggerChain::default());
    self.process_delete(obj, &ctx).await;   // processes INLINE, on the caller's task
}
```

All three conf-center backends call `on_delete` from their event loop, so for any kind **two tasks
can execute `process_delete` in parallel**: the backend event loop, and the workqueue worker (which
reaches `process_delete` via `process_work_item`'s `(None, Some(cached))` arm, `processor.rs:444`).
On a multi-threaded runtime these run on different threads at once. `process_delete` has exactly two
call sites — `processor.rs:379` (the bypass) and `processor.rs:444` (the worker).

This contradicts the code's own documented design. `kubernetes/resource_controller.rs` opens with:

```
//! - Go operator-style workqueue: ALL events enqueue key, worker decides update/delete
```

and line 428's comment reads `// Runtime phase - enqueue key for worker` directly above a call that
does not enqueue.

## How it surfaced

This parallelism is what allowed two Gateway deletes to reach the event store out of version order,
where a per-kind stale guard silently discarded the older-numbered one. See
`docs/superpowers/specs/2026-07-17-conf-sync-event-ordering-design.md`.

**That incident is fixed separately** by allocating `sync_version` inside the store write lock, which
makes ordering correct regardless of how many tasks race. This task is the remaining architectural
cleanup, not a live correctness bug.

## Why it was not fixed alongside

The obvious fix — make `on_delete` enqueue, letting `process_work_item`'s existing
`(None, Some(cached)) => process_delete` arm handle it — looks like a three-line change. Adversarial
review found three problems that make it its own project:

1. **FS conflict-loser promotion.** `PathRegistry::forget()` on a winner *promotes* a conflict loser
   — asserted by its own test at `path_registry.rs:379-380` (*"conflict file is still live —
   winner_of must surface it"*). The FS worker computes `path = registry.winner_of(&rkey)`
   (`file_system/resource_controller.rs:524-528`), would get the surviving loser, and take
   `(Some(obj), _)` → **re-apply instead of delete**. Requires a duplicate-key file; arguably the
   more correct level-triggered behavior, but untested.

   K8s and etcd are clean: kube-runtime's `reflector()` applies the event to the store
   (`reflector/mod.rs:127`) before yielding it (`:129`); etcd's `fs_raw_store.remove(&rkey)`
   (`etcd/resource_controller.rs:213`) precedes `on_delete` (`:215`).

2. **delete→create coalescing skips `handler.on_delete` → ref leak.** The queue coalesces by key, so
   a delete followed by a recreate of the same key resolves to `(Some(obj), _)` → `process_resource`
   and `on_delete` never runs. `RefManager::add_ref` is additive (`ref_manager.rs:191`) and
   `Gateway::parse` only ever adds (`gateway.rs:251,265,291,346,893,900`); only `on_delete` clears
   (`gateway.rs:527-545`). Old-generation refs leak into the new one. The same leak already exists on
   a plain update, so it is not a new class — but this change removes a cleanup point that currently
   makes recreate clean. Likely fix: have `parse` clear source refs before re-adding.

3. **Delete inherits queue backpressure.** Today `on_delete` bypasses the queue, so a delete always
   makes progress. After the change, deletes queue behind everything — including the worker path's
   status IO (`tasks/todo/worker-path-blocking-status-io.md`). `enqueue()` awaits `tx.send()` on a
   bounded mpsc (capacity 5000, `workqueue.rs:350,552`). A wipe-all moves 1091 deletes from "inline,
   parallel" to "serial, behind status IO", and the `cache_residue` check has a **30s timeout** —
   there is a plausible path from the current hard failure to a timeout failure. **Measure before
   landing.**

## Also note

The call sites are not "unchanged". FS (`file_system/resource_controller.rs:391`) and etcd
(`etcd/resource_controller.rs:214`) gate the delete on `if let Some(obj) = processor.get(&key)` —
they already pass the *cached* object, not the event object (only K8s passes the event object,
`kubernetes/resource_controller.rs:430`). Left in place, those guards would skip enqueuing for a key
present in the store but absent from cache. They must go.

## Related

- `docs/superpowers/specs/2026-07-17-conf-sync-event-ordering-design.md`
- `tasks/todo/agents-md-workqueue-discipline.md` — the rule this violates; land them together
- `tasks/todo/worker-path-blocking-status-io.md` — what deletes would queue behind
- `tasks/todo/initdone-double-spawn-second-worker.md` — the other threat to single-worker-per-kind

## Closing note (2026-07-17)

Fixed by `docs/superpowers/specs/2026-07-17-on-delete-workqueue-design.md`:
`on_apply`/`on_delete` merged into `on_event_key`; all six backend call sites
enqueue; the FS/etcd cache guards are gone. Two claims above were found stale
during design review: (1) the FS conflict-loser promotion scenario cannot
reach the controller — `PathRegistry::forget()` runs in the watcher before
dispatch and emits a synthetic Apply for a promoted loser (Delete only on full
removal); (2) `Gateway::parse` has been clear-before-add for all four ref
managers since commit 82fcfda3. The backpressure concern was measured (see the
spec appendix) and the delete path no longer performs a status read.
