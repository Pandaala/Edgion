# A second InitDone spawns a second worker on the same workqueue

**Status**: todo — dormant, not currently reachable
**Priority**: P3 — no live bug; it makes "one worker per kind" a convention rather than a guarantee
**Filed**: 2026-07-17, found by adversarial review of the conf_sync ordering design

## The defect

All three conf-center backends assign the worker handle unconditionally inside their `InitDone` arm,
with no `init_done` guard:

- `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs:370`
- `edgion-controller/src/conf_mgr/conf_center/file_system/resource_controller.rs:261`
- `edgion-controller/src/conf_mgr/conf_center/etcd/resource_controller.rs:165`

```rust
worker_handle = Some(spawn_worker(...));
```

Reassigning an `Option<JoinHandle>` **drops** the old handle — and dropping a tokio `JoinHandle` does
**not** abort the task. It detaches it. So a second `InitDone` leaves two live workers pulling from
one workqueue.

## Why FS and etcd are exposed and K8s is not

K8s breaks the loop on a second `Event::Init` (`kubernetes/resource_controller.rs:316-332`), so a
second InitDone cannot follow.

FS and etcd do not:
- FS logs *"Received Init event after init done, ignoring"* and **falls through**
  (`file_system/resource_controller.rs:129-146`).
- etcd ignores it silently (`etcd/resource_controller.rs:108-110`).

FS's own code therefore *anticipates* a second Init and then spawns a second worker on the InitDone
that follows it.

## Why it is dormant today

`init_phase()` is called exactly once per controller run (`file_watcher.rs:111`), and a config reload
constructs a fresh `FileSystemController` rather than re-initialising the existing one
(`file_system/center.rs:128-139`). So no second InitDone is emitted in practice.

## Why it still matters

Two workers on one queue would process the same kind concurrently, which breaks the serialization
that the whole processing model assumes. The 2026-07-17 design
(`docs/superpowers/specs/2026-07-17-conf-sync-event-ordering-design.md`) deliberately does **not**
rely on single-worker serialization for correctness — it allocates `sync_version` inside the store
lock so ordering holds regardless — precisely because this guarantee does not exist.

It would also silently defeat the completed `on_event_key` workqueue design: that fix's entire
premise is "everything flows through one serial worker".

## Fix sketch

Guard the spawn on the handle being vacant, or on `!init_done`:

```rust
if worker_handle.is_none() {
    worker_handle = Some(spawn_worker(...));
}
```

and decide, per backend, whether a second Init should be a hard error rather than a warning. FS's
"ignoring" log is misleading — it ignores the Init and then acts on the InitDone.

## Related

- `docs/superpowers/specs/2026-07-17-conf-sync-event-ordering-design.md`
- `docs/superpowers/specs/2026-07-17-on-delete-workqueue-design.md` — depends on
  single-worker-per-kind actually holding
- `AGENTS.md` (`Workqueue discipline (controller conf_sync)`) — the rule this silently breaks

## Addendum (2026-07-17)

A third instance beyond FS/etcd double-Init: K8s **multi-namespace** mode —
`merge_namespaced_watchers` deliberately passes through post-completion
`InitDone`s (`kubernetes/resource_controller.rs:133`) and the `InitDone`
branch unconditionally spawns a worker (`:370`). Since the on_delete-workqueue
change, deletes also ride the possibly-doubled worker.
