# ctrl3-02: FileSystem/etcd backends do not supervise kind workers, and a post-init backend error parks the Controller as a non-ready zombie

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs:352-363` (`LoopAction::Error` after init → `shutdown_handle.signal().wait().await`)
  - `edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs:489-614` (`spawn_all_controllers` returns kind handles nobody polls)
  - `edgion-controller/src/conf_mgr/conf_center/file_system/controller.rs:158-180` and `etcd/controller.rs:138-158` (handles only `abort()`ed after `run_runtime` returns)
  - `edgion-controller/src/conf_mgr/conf_center/file_system/resource_controller.rs:311-320`, `:530-563` and `etcd/resource_controller.rs:212-220`, `:280-300` (worker handle only awaited at shutdown)
  - `edgion-controller/src/conf_mgr/conf_center/file_system/file_watcher.rs:402-411`, `:502-508` (sequential dispatch, `sender.send().await` on a 4096-slot channel)
  - `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:184` (queue capacity 5000)
  - `edgion-controller/src/cli/mod.rs:473-507` (deadman fires only when the lifecycle future ends)
  - Contrast: `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs:494-498`, `:548-553` (worker death → `bail!` → epoch restart)

## Summary

In Kubernetes mode a dead per-kind worker is detected (`select!` on the worker handle) and the
resource epoch is rebuilt; after repeated failures the process shuts down so Kubernetes restarts it.
The FileSystem and etcd backends have neither property. A panicking handler kills that kind's
worker silently while readiness stays green; its events keep being enqueued (deduplicated) until
the bounded queues fill, after which the shared FS watcher blocks and *every* kind stops
reconciling. And any
backend error after initialization tears down the ConfigSync server and then waits forever for an
external shutdown, so the process stays alive, `/health` stays 200 and nothing restarts it.

## Current behavior (evidence)

- Worker death is invisible locally: `FileSystemResourceController::run` stores the worker in
  `worker_handle` (`resource_controller.rs:311-320`) and only touches it again in the shutdown path
  (`:530-563`); the event loop never selects on it. The etcd controller is identical
  (`etcd/resource_controller.rs:212-220`, `:280-300`).
- After the worker is gone, runtime events still call `self.processor.on_event_key(key).await`
  (FS `:464`, etcd `:338`). Keys are deduplicated, so the kind first just stops reconciling
  silently; the workqueue is a bounded channel (`capacity: 5000`, `workqueue.rs:184`, `:339`,
  `self.tx.send(..).await` at `:534`), so once more than 5000 distinct keys are pending the kind
  controller blocks; its 4096-slot event channel
  (`file_watcher.rs:34`, `:92`) fills; `dispatch_file_event` blocks in `sender.send().await`
  (`:502-508`) inside the single sequential watcher loop (`:402-411`), so all kinds stall.
  `CenterBase::is_ready` (`non_kube_common.rs:169-171`) only checks the registry and CSS presence,
  so readiness remains true throughout.
- Kind-task failures are also unobserved: `spawn_all_controllers` returns `JoinHandle`s that the FS
  and etcd controllers never await — they are only aborted after `watcher.run_runtime` returns
  (`file_system/controller.rs:175-180`, `etcd/controller.rs:154-158`).
- Post-init backend error: when the backend's `run()` returns `Err` (for example the FS watcher
  reports "FS runtime receiver closed for kind" once a kind task ended), `run_lifecycle_gated`
  gets `LoopAction::Error`, calls `stop_iteration` (CSS set to `None`, runtime reset,
  `:321-325`), then, because `initialized` is true, logs and executes
  `shutdown_handle.signal().wait().await; return Ok(())` (`:352-363`). The lifecycle future never
  completes, so the deadman in `cli/mod.rs:473-507` (`mark_lifecycle_terminated` +
  `shutdown()`) never runs.

## Industry comparison

- The Kubernetes path of this same codebase already adopted the controller-runtime pattern:
  supervise every worker, restart the epoch, and exit after a failure budget
  (`skills/04-review/controller/regression-guards/liveness-probe-and-lifecycle-supervision.md`).
- controller-runtime and kube-rs `Controller` treat a reconciler panic as a crash of the
  manager (process exits, pod restarts); APISIX/Kong in standalone file mode exit on fatal
  config-loader errors rather than idling.

## Impact

- FileSystem/etcd deployments can silently stop applying configuration (one kind, then all kinds)
  while reporting ready — gateways keep serving stale config with no signal besides logs.
- A post-init backend error leaves a permanently not-ready but live process: no automatic
  restart (liveness is 200), Admin API still answering, etcd mode possibly still holding the
  leadership lease via the independent election task.

## Suggested direction

1. Select on the worker handle (and on kind task handles in `FileSystemController`/
   `EtcdController::run`) exactly as the Kubernetes `run_sources` loop does, and convert a dead
   worker/kind task into a backend error.
2. Replace the "wait for shutdown" branch in `run_lifecycle_gated` with the Kubernetes behavior:
   bounded in-process restarts of the iteration (the loop already supports re-running
   `make_handle`), then `shutdown_handle.shutdown()` once a failure budget is exhausted so the
   existing deadman/exit path applies.
3. Optionally reuse the K8s `spawn_observed`/`leader_job_alive` style gauge for per-kind workers so
   operators can alert.

## Notes

- `liveness-probe-and-lifecycle-supervision.md` records "Give-up-without-shutdown was itself a
  wedge" and fixes it for Kubernetes only; its "Local backend iteration teardown" section covers
  teardown/drain of local workers (ISSUE-005), not death detection or the post-init give-up branch.
  It rejects auto-restarting *leader jobs* and `catch_unwind` in callees; this proposal uses the
  same JoinHandle-boundary observation and epoch restart that page endorses for Kubernetes.
- `ctrl-center-02` covers the hard-coded init-phase timeout, not runtime supervision.
- Severity P2 because FileSystem/etcd are secondary backends (`ctrl-center-08`), but the failure is
  silent.

## Verification (independent)

- **Verdict**: CONFIRMED (P2)
- `non_kube_common.rs:352-363`: after init, `LoopAction::Error` logs "Controller stopped with error, waiting for shutdown" and parks on `shutdown_handle.signal().wait().await`, after `stop_iteration` already reset the runtime/CSS (`:321-325`) - no restart and no `shutdown()` call, so the CLI deadman never fires.
- `file_system/controller.rs:155-180`: kind handles from `spawn_all_controllers` are only `abort()`ed after `watcher.run_runtime` returns; nothing polls them.
- `file_system/resource_controller.rs:311-320`: the runtime worker is stored in `worker_handle` and never selected on, unlike `kubernetes/resource_controller.rs:494-497`, which `bail!`s on worker exit.
- No `panic = "abort"` profile exists in any `Cargo.toml`, so a handler panic does unwind and kill only the worker task; workqueue capacity is 5000 (`workqueue.rs:184`).
- `CenterBase::is_ready` (`non_kube_common.rs:169-171`) checks only processor readiness and CSS presence, not worker liveness.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `non_kube_common.rs`, `file_system/controller.rs`, `file_system/resource_controller.rs`, `etcd/*controller.rs` and `workqueue.rs` are unchanged on this branch; cited lines are current.
- `non_kube_common.rs:352-363` still logs "Controller stopped with error, waiting for shutdown" and parks on `shutdown_handle.signal().wait().await`.
- Branch changes under `conf_center/` touch only `common.rs`, status/storage/source_query and kube leader election, not local-backend supervision.
