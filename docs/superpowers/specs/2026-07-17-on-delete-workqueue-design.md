# Design: route `on_delete` through the workqueue

**Date**: 2026-07-17
**Status**: implemented (2026-07-18)
**Task**: `tasks/todo/on-delete-bypasses-workqueue.md`
**Lands together with**: `tasks/todo/agents-md-workqueue-discipline.md`
**Related**: `docs/superpowers/specs/2026-07-17-conf-sync-event-ordering-design.md`

## Problem

`ResourceProcessor::on_apply` enqueues a key for the kind's single worker, but
`ResourceProcessor::on_delete` runs `process_delete` inline on the backend event-loop task
(`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:371-380`). All three
conf-center backends call it from their event loops, so for any kind two tasks can execute
`process_delete` in parallel: the backend event loop and the workqueue worker (via
`process_work_item`'s `(None, Some(cached))` arm). This contradicts the documented design
("Go operator-style workqueue: ALL events enqueue key, worker decides update/delete") and is the
parallelism that enabled the 2026-07-17 cache-residue incident. That incident is already fixed
separately (sync_version allocated inside the store write lock); this change removes the
architectural asymmetry itself.

## Decisions

1. **Scope**: core fix + write the workqueue discipline rule into AGENTS.md in the same batch.
   The status-IO and double-worker problems stay in their own tasks
   (`worker-path-blocking-status-io.md`, `initdone-double-spawn-second-worker.md`).
2. **API shape**: merge `on_apply`/`on_delete` into a single key-only entry point. After the change
   the two bodies would be identical; two names implying different semantics is exactly the illusion
   this fix removes. Pre-release, no compatibility burden.
3. **FS conflict-loser promotion**: reviewed and found to be a non-issue — the scenario the task
   file describes cannot reach the controller (see below). No behavior change on this axis.
4. **delete→create coalescing**: handled by auditing every handler for full-replacement discipline
   in the apply path — covering both registry state (clear-before-add) *and* requeue notifications
   to old-generation dependents — plus coalescing tests.
5. **Backpressure**: skip the `.status` read on the delete path unconditionally (safe by
   construction), and measure the wipe-all scenario before landing; further mitigation only if the
   measurement crosses the threshold below.

## Core change

### `ResourceProcessor` (generic — one change covers all ~25 kinds)

Replace `on_apply(&K)` and `on_delete(&K)` with:

```rust
/// Enqueue a watch-event key for the worker (apply and delete alike —
/// the worker compares store state vs cache and decides update/delete).
pub async fn on_event_key(&self, key: String) {
    self.workqueue.enqueue(key).await;
}
```

- Enqueue-failure semantics match today's `on_apply` (return value ignored). Note this is a
  (deliberate) semantic change for deletes at shutdown: today an inline delete always executes;
  after the change, an enqueue on a closed queue logs and drops (`workqueue.rs:563-575`). Only
  reachable during teardown; accepted.
- `process_delete` is already private (`processor.rs:658`); the change deletes its bypass call site
  at `:379`, leaving exactly one call site — `process_work_item`'s `(None, Some(cached))` arm.
  Verified by grep, recorded in the acceptance criteria.

### Backend call sites (6 total, in 3 generic backend event loops)

- **Kubernetes** (`conf_mgr/conf_center/kubernetes/resource_controller.rs:416,430`): both Apply and
  Delete branches compute `make_resource_key(&obj)` and call `on_event_key`. Each branch keeps its
  existing `passes_namespace_filter` check (`watcher::Event::Delete` carries the full final object,
  so the key is always computable). The line-428 comment
  `// Runtime phase - enqueue key for worker` becomes true.
- **FileSystem** (`conf_mgr/conf_center/file_system/resource_controller.rs:386-401`): after
  `parse_delete_info` yields the key, keep `fs_raw_store.remove(&key)`, then call
  `on_event_key(key)` directly. Delete the `if let Some(obj) = processor.get(&key)` guard and its
  "Delete event for non-cached resource, ignoring" arm — the guard skips keys present in the store
  but absent from cache; the worker's `(None, None)` arm already handles a truly-gone key
  (`WorkItemResult::Skipped`). Update both stale module-header comment lines
  (`resource_controller.rs:10-11`).
- **etcd** (`conf_mgr/conf_center/etcd/resource_controller.rs:210-218`): same as FileSystem — keep
  `fs_raw_store.remove`, delete the cache guard, enqueue the key.
- **Delete-path status read**: the worker currently performs `read_status_json` before
  `process_work_item` unconditionally (FS `:628`; etcd `:320` — a network RPC in etcd mode), but
  `existing_status_json` is only consumed on the `(Some(obj), _)` arm. Skip the read when it can be
  skipped safely — deletes must not pay a status read. (Exact mechanism — e.g. read lazily or only
  when the store object exists — is an implementation detail; the requirement is: no status read on
  the delete path.)

### Restored invariant

Apply and delete for one key now flow through the same queue and the same single worker, serially.
The backend event loop never touches handlers directly at runtime. This is the property whose
absence caused the 2026-07-17 incident; the lock-scoped `sync_version` allocation stays in place as
a second, independent layer of defense.

**Init-phase exception (by design)**: during init (pre-worker, single task) the backends call
`on_init_apply` → `process_resource` directly (`processor.rs:348-355`). The serial invariant holds
trivially there; the AGENTS.md rule below carves this out explicitly.

### Bonus fix: status cleanup on delete

Today the inline delete path never deletes the `.status` sidecar — FS (`:386-401`) and etcd
(`:210-218`) Delete branches call no status deletion; only the worker's `Deleted` arm does
(FS `:685-696`, etcd `:356-358`). Routing deletes through the worker means resource deletion now
also cleans up its status file/key. This is claimed and tested (see test plan).

Residual (unchanged from today, explicitly out of scope): a never-cached key — e.g. a file that
always failed parse and therefore has a `ParseError` `.status` file — resolves to
`(None, None) → Skipped` and leaves the orphan status file behind, exactly as today.

## Known-problem resolutions

### FS conflict-loser promotion — non-issue (task file corrected)

The task file's problem 1 does not occur. `PathRegistry::forget()` runs in the **watcher**, before
event dispatch (`file_watcher.rs:521-555`): `ForgetOutcome::Promoted` emits a **synthetic Apply
carrying the promoted loser's content**; a `FileSystemEvent::Delete` is emitted only on
`ForgetOutcome::Removed` (no surviving conflict file). So by the time a delete work item is
dequeued, `winner_of` is `None` and the worker observes `(None, cached)` → delete. Deleting a
conflict winner already surfaces the loser today via Apply → queue, identically before and after
this change. The `path_registry.rs:379-380` test cited by the task file concerns
`mark_api_deleted`, a different mechanism that also never produces a controller-visible Delete with
a surviving winner.

The only way a delete work item's re-read finds `(Some(obj), _)` is a delete→recreate race (a new
file recorded between `forget` and dequeue) — re-apply is then the correct level-triggered outcome.
That race belongs to the coalescing analysis below.

### delete→create coalescing (handler audit + tests)

The queue coalesces by key (`workqueue.rs:526-577`: a `pending` DashMap dedups; `WorkItem` carries
only key + trigger chain, no object and no event type), so a delete followed by a recreate before
dequeue resolves to `(Some(obj), _)` and `handler.on_delete` never runs. The discipline is
therefore two-fold — **the apply path must fully replace every side effect of the previous
generation**:

1. **Registry state**: clear-before-add on every registry the handler writes (ref managers,
   `listener_port_manager`, `gateway_route_index`, `attached_route_tracker`, …).
2. **Requeue notifications**: the apply path must requeue every dependent whose derived state
   changes — **including dependents referenced only by the previous generation**. Worked example:
   `HTTPRoute::on_delete` requeues the *old* parent Gateways (`http_route.rs:466-469`), old
   delegation children/parents (`:437-463`); `on_change` (`:405-423`) requeues only the *new*
   spec's parents. A coalesced delete→recreate whose new spec drops Gateway A would leave A's
   `attachedRoutes` stale until A is reprocessed for an unrelated reason. Registry state is fine
   (route registries are replacement-style, `route_utils.rs:405,412`); it is the *requeue* that is
   lost. (A plain update already has this class of leak; the audit fixes the class, not just the
   coalescing instance.)

- Audit all ~25 handlers in `conf_mgr/sync_runtime/resource_processor/handlers/` against both
  criteria. The audit table (one row per handler; columns: side-effect registries, delete-only
  requeue targets, verdict, fix) goes in the implementation PR description. 20 handlers implement
  `on_delete`; several (e.g. `endpoint_slice`, `service`) have pure requeue-notification delete
  paths — exactly criterion 2's territory. `Secret::on_delete`'s `clear_source_refs`
  (`secret.rs:194-215`) needs per-handler judgment for the "both sides deleted simultaneously"
  window (bounded today — dependents self-clean on reconcile).
- Note: the task file's claim that `Gateway::parse` only ever adds is stale — since commit
  `82fcfda3` it clears all four ref managers before re-adding (`handlers/gateway.rs:236-276`), and
  `listener_port_manager.register_gateway` is replacement-style. Gateway is compliant on criterion
  1; the audit covers the rest.

### Backpressure (measure before landing)

Deletes now inherit queue backpressure. Direction check: this mostly *helps* the event loop — today
it executes the whole `handler.on_delete` + cache write inline per delete; after the change it does
one channel send (blocking only at queue depth ≥ 5000; wipe-all is 1091 deletes). A faster-draining
event loop *reduces* FS broadcast-`Lagged` risk (capacity 4096, no recovery —
`file_system/resource_controller.rs:406-418`). The residual worse case is narrow: queue already
full from an apply storm while the worker stalls on status IO — then deletes stall too, where today
they would progress.

- **Method**: run the existing `cache_residue` wipe-all integration scenario (1091 deletes) on the
  pre-change baseline and on the change, 3 runs each; record median convergence time, peak queue
  depth, and any FS `Lagged` warnings.
- **Gate**: post-change median < 15s (half the `cache_residue` 30s timeout) → land. Otherwise
  evaluate further minimal mitigation inside this change (the delete-path status-read skip is
  already unconditional per the core change); the full status-writer extraction remains
  `worker-path-blocking-status-io.md`'s scope.
- The measurement protocol is fixed here; the numbers are recorded in the appendix before landing
  (this is the landing gate, not an open design question).

## AGENTS.md: "Workqueue discipline" section

Add a section stating the rule in full, not weakened to match the code:

1. Every resource kind has exactly **one** workqueue and **one** worker.
2. Dequeue processing is **serial** and must not wait.
3. **Every** runtime event — apply *and* delete — enqueues a key. Nothing calls `process_*`
   directly; the entry point is `on_event_key`. *Sanctioned exception*: the init phase
   (`on_init_apply` → `process_resource`) runs pre-worker on a single task, where the serial
   invariant holds trivially.
4. Anything needing reprocessing goes through **requeue**.
5. Handler processing must contain **no API calls, timers, sleeps, or other latency**.

With a named list of known outstanding violations (do not pretend the code complies):

- Rule 5: status IO inside the worker path → `tasks/todo/worker-path-blocking-status-io.md`
- Rule 1: one-worker-per-kind is a convention, not a guarantee →
  `tasks/todo/initdone-double-spawn-second-worker.md`; besides FS/etcd double-Init, K8s
  **multi-namespace** mode double-spawns too — `merge_namespaced_watchers` passes through
  post-completion `InitDone`s (`kubernetes/resource_controller.rs:133`) and the `InitDone` branch
  unconditionally spawns a worker (`:370`).

The former violation of rule 3 (`on_delete` bypassing the queue) is eliminated by this change.

## Test plan

1. **FS sole-file delete (new path)**: delete the only file for a key → assert worker-side delete
   of the resource **and removal of its `.status` file** (locks the status-cleanup fix). Baseline
   note: the previously proposed "delete conflict winner → loser applies" test exercises the
   watcher's synthetic-Apply path and passes with or without this change; it may be added for
   watcher coverage but does not validate this design.
2. **Delete→recreate race (FS)**: new file for the same key recorded between Delete dispatch and
   dequeue → assert re-apply (level-triggered), no delete side effects.
3. **Coalescing with changed dependents**: delete + recreate an HTTPRoute with *changed parentRefs*
   before the worker dequeues → assert the dropped parent Gateway's `attachedRoutes`/status
   converges (this is the oracle that catches the lost-requeue class; RefManager queries alone
   cannot).
4. **Guard removal**: delete of a never-cached key (FS and etcd) → `(None, None)` Skipped, no
   status write, no panic.
5. **Existing suites**: conf-sync integration tests (reconnect, `cache_residue`, …) all green;
   `cargo check/clippy/fmt`.
6. Shutdown-time delete drop (P3) is waived explicitly: teardown-only, no test.

## Acceptance criteria

- `process_delete` has exactly one call site (the worker arm) — grep-verified; it remains private.
- All three backends enqueue on delete; the FS/etcd `processor.get` guards are gone; both FS
  header-comment lines updated.
- Delete path performs no `.status` read.
- Test plan items 1–5 pass.
- Handler audit table (both criteria) complete in the implementation PR; violations fixed.
- Wipe-all benchmark recorded in the appendix; post-change median < 15s; peak queue depth and
  `Lagged` count recorded.
- AGENTS.md rule (with init carve-out and the three-instance rule-1 violation note) lands in the
  same batch; both task files (`on-delete-bypasses-workqueue.md`,
  `agents-md-workqueue-discipline.md`) are closed, and the task file's stale claims (FS promotion,
  `Gateway::parse` only-adds) are corrected in the closing note.

## Risks

- **Delete latency**: negligible in steady state (one queue hop); the wipe-all extreme is covered
  by the measurement gate. Direction favors the change (event loop drains faster; see
  backpressure section).
- **Lost delete-only requeues under coalescing**: converts a currently-clean path (delete+recreate)
  into the same staleness class a plain update already has. Mitigated by audit criterion 2, which
  fixes the class at the root; locked by test 3.
- **Shutdown**: a delete arriving during teardown may be dropped by a closed queue (today it would
  run inline). Accepted; teardown-only.
- **Residual double-worker threat**: until `initdone-double-spawn-second-worker.md` is fixed
  (now including the K8s multi-namespace instance), a second worker can break the serial
  invariant. Out of scope here; named in the AGENTS.md violations list.

## Appendix: wipe-all benchmark (landing gate — filled at implementation time)

| Run | Baseline (pre-change) | Post-change | Peak queue depth | FS Lagged warnings |
|-----|----------------------|-------------|------------------|--------------------|
| 1   | 0.953s               | 1.268s      | 0 high-water warns | 0 |
| 2   | 0.844s               | 0.815s      | 0 high-water warns | 0 |
| 3   | 1.405s               | 1.036s      | 0 high-water warns | 0 |
| Median | 0.953s            | 1.036s      | 0 high-water warns | 0 |

**GATE: PASS** — post-change median 1.036s is far below the 15s threshold (half the
`cache_residue` 30s timeout) and statistically indistinguishable from the 0.953s baseline
(both ranges overlap: baseline 0.844-1.405s, post-change 0.815-1.268s). The peak-queue-depth
and Lagged columns apply to both variants: 0 high-water warns and 0 `Event receiver lagged`
warns in all 6 runs.

**Baseline run conditions**: `edgion-tests/integration/scripts/integration/run_integration.sh`, full
suite (fast-test mode, no `--full-test`), 3 runs on commit `0543830a` on `feature-06-24` (verified
pre on_delete-workqueue change: `on_delete` at
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:377-379` still calls
`process_delete` inline from the backend event loop instead of enqueueing, and
`edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs` is unmodified since `f268837d`). Note:
commit `00964f6d` (between `f268837d` and `0543830a`, landed hours before these runs) is the
cache-residue "sync_version drawn inside the store write lock" fix this design's Problem section
already describes as fixed separately — it is not the workqueue-routing change itself and does not
affect this baseline's validity. Each run's wipe-all reported `total=1114 errors=0` (deletes across
all loaded integration scenarios' resources — not the 1091 estimated in the Backpressure section
above; the discrepancy is scenario-set drift since that estimate was written, not a measurement
error). Run 1's first attempt hit unrelated Redis-client test timeouts (`LinkSys_Redis`,
`EdgionPlugins_Dsl` — Docker Redis container contention, unrelated to wipe-all/delete processing)
while the wipe-all/cache-residue measurement itself still passed; per the flaky-retry policy it was
re-run once and passed cleanly (`All tests passed!`). Runs 2 and 3 passed cleanly on the first
attempt.

**Extraction method (reuse identically for the post-change column)**:

1. Client-observed wipe duration + resource count — from the `tee`d suite stdout/stderr log:
   ```bash
   grep "wipe-all: total=" "$SCRATCHPAD/bench-baseline-runN.log"
   ```
   This is the server-side ConfCenter delete-call duration (`wipe_handlers::wipe_all`'s own
   `Instant` timer), NOT full downstream convergence — kept only as a sanity cross-check (all 3
   runs: `total=1114 errors=0`, `duration_ms` in the 253–397ms range).

2. **Path correction to the brief**: the controller's tracing log is NOT under
   `<TEST_LOG_DIR>/controller.log` (`TEST_LOG_DIR` = `${WORK_DIR}/test_logs`, which holds only
   per-scenario test-client logs). The controller's own log lives at
   `${WORK_DIR}/logs/edgion-controller.log` (plain text) — use this one, not
   `${WORK_DIR}/logs/controller.log` (ANSI-colored copy of the same content; only useful for human
   terminal viewing). `WORK_DIR` for a given run is printed by the suite as `Test log directory:
   <WORK_DIR>/test_logs` and by the `[INFO] Workdirectory: <WORK_DIR>` cleanup line; strip the
   `/test_logs` suffix (or the cleanup line's value is `WORK_DIR` directly).

3. Warn counts (both 0 across all 3 baseline runs — wipe-all's 1114 deletes never approached the
   workqueue's 5000-depth high-water threshold or the FS watcher's 4096-capacity broadcast
   channel):
   ```bash
   grep -c "high-water" "${WORK_DIR}/logs/edgion-controller.log"          # 0 in all 3 runs
   grep -c "Event receiver lagged" "${WORK_DIR}/logs/edgion-controller.log"  # 0 in all 3 runs
   ```

4. **Convergence time — the brief's suggested pattern (`debug!("Resource deleted from cache")` in
   `processor.rs:679`) does not fire**: the integration suite's controller runs at `log_level: info`
   (`config/edgion-controller.yaml:77`), so that `tracing::debug!` line is never emitted, and no
   `tracing-subscriber` span-close-event logging is configured either (`process_delete`'s
   `info_span!` never auto-logs on close). The reliable INFO-level substitute found by inspection:
   every delete that reaches the controller's cache store logs `apply change ... event="apply_change"
   change=EventDelete ... sync_version=N` from `conf_sync::cache_server::cache:388`, and — because
   it runs inside the `process_delete` span — carries a `process_delete{...}` span prefix in the
   `tracing-subscriber` fmt output. Its count restricted to lines at-or-after the `wipe_all started`
   line matched the wipe-all `total` exactly in all 3 runs (1114 == 1114), confirming it captures
   every wipe-all-triggered delete with no under/over-count. Exact commands:
   ```bash
   # Start: the (last, in case of multiple wipe-all calls in one log) "wipe_all started" line
   WIPE_START_LINE=$(grep -n 'wipe_all started' "${WORK_DIR}/logs/edgion-controller.log" | tail -1 | cut -d: -f1)
   WIPE_START_TS=$(sed -n "${WIPE_START_LINE}p" "${WORK_DIR}/logs/edgion-controller.log" | grep -oE '^[0-9T:.-]+Z')

   # End: the last EventDelete cache-apply line at or after that point
   LAST_DELETE_TS=$(awk -v start="$WIPE_START_LINE" \
     'NR>=start && /event="apply_change" change=EventDelete/{line=$0} END{print line}' \
     "${WORK_DIR}/logs/edgion-controller.log" | grep -oE '^[0-9T:.-]+Z')

   # Sanity check: count must equal wipe-all's reported total
   awk -v start="$WIPE_START_LINE" \
     'NR>=start && /event="apply_change" change=EventDelete/{c++} END{print c}' \
     "${WORK_DIR}/logs/edgion-controller.log"   # must print 1114

   python3 -c "
   from datetime import datetime
   fmt = '%Y-%m-%dT%H:%M:%S.%fZ'
   a = datetime.strptime('$WIPE_START_TS', fmt)
   b = datetime.strptime('$LAST_DELETE_TS', fmt)
   print((b - a).total_seconds())
   "
   ```
   Raw timestamps used:
   - Run 1: start `2026-07-17T14:34:57.174921Z` → last delete `2026-07-17T14:34:58.127486Z` → 0.952565s
   - Run 2: start `2026-07-17T14:41:57.095459Z` → last delete `2026-07-17T14:41:57.939415Z` → 0.843956s
   - Run 3: start `2026-07-17T14:47:27.357937Z` → last delete `2026-07-17T14:47:28.762455Z` → 1.404518s

   Caveat: this measures controller-side cache-store convergence only (the on_delete-workqueue
   change's scope), not gateway-side gRPC propagation — consistent with the design's problem
   statement, which is entirely about the controller's `resource_processor`/workqueue path.

**Post-change run conditions**: same suite, 3 runs on commit `c2eeb174` (all 7 implementation
commits included). Extraction method identical to the baseline (same `wipe_all started` →
last-`EventDelete` timestamps from `${WORK_DIR}/logs/edgion-controller.log`; sanity count matched
`total=1114` exactly in all 3 runs). Raw timestamps used:
   - Run 1 (workdir `testing_20260718_075322`): 1.267531s
   - Run 2 (workdir `testing_20260718_081713`): 0.815254s
   - Run 3 (workdir `testing_20260718_094637`): 1.035616s; `All tests passed!`
Run 3 required three environment recoveries before a clean run, none related to this change: a
host reboot dropped the `lo0` loopback aliases (127.0.0.2-4), then post-reboot cold-start load
(~14 loadavg: Spotlight + OrbStack K8s churn) expired the Dyn_* scenarios' hot-reload wait
windows, then a concurrent integration environment in a different workspace (ws3) held shared
test ports and its name-based cleanup killed this workspace's processes (see
`tasks/todo/kill-all-processes-matches-too-much.md`). In the failed-under-load runs the
controller and gateway logs showed correct config processing throughout — listeners were created
correctly, just after the scenarios' probe windows.
