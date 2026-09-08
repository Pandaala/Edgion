---
name: single-worker-initialization
description: Use when reviewing per-kind controller initialization completion, worker ownership, or repeated watcher lifecycle events.
knowledge_type: regression-guards
---

# Initialization completion starts only one worker

## Conclusion and scope

The FileSystem and etcd resource-controller event loops accept
`InitDone` only once per run. Their local `init_done` boolean guards the entire
completion branch before the timer, `ResourceProcessor::on_init_done`, readiness
notification where present, and `spawn_worker`. A duplicate completion warns and
continues without replacing `worker_handle: Option<JoinHandle<()>>`.

Kubernetes now guards completion at each native source Writer and separately
performs one committed-union bootstrap before starting its worker. Runtime
source completions resync keys without repeating initialization. See
[source-scoped snapshots](source-scoped-watch-snapshots.md) for that broader
correction and its implementation verification status.

The processor owns the resource kind's `Arc<Workqueue>`; each spawned worker
holds an `Arc<ResourceProcessor<K>>` and consumes that same queue. Preserving
the first task handle avoids detaching a still-running worker. This enforces
the existing single-worker requirement in `AGENTS.md`, Workqueue discipline,
for repeated completion events; it is not a new worker supervision design.

## Rationale and evidence

The original `initdone-double-spawn-second-worker` finding identified
unconditional `worker_handle = Some(spawn_worker(...))` in all three backends.
Dropping the replaced Tokio `JoinHandle` detaches the old task instead of
stopping it, leaving multiple consumers and losing its join handle.

Source locations, relative to `edgion-controller/src/conf_mgr/conf_center/`:

- `file_system/resource_controller.rs`: `FileSystemResourceController::run`,
  `FileSystemEvent::InitDone`, and `spawn_worker`.
- `etcd/resource_controller.rs`: `EtcdResourceController::run`,
  `EtcdEvent::InitDone`, and `spawn_worker`.
- Historically, `kubernetes/resource_controller.rs`:
  `ResourceController::run_with_stream`, `Event::InitDone`,
  `merge_namespaced_watchers`, and `spawn_worker`. The merger and combined
  reflector were replaced by `run_sources` and `watch_source.rs::SourceState`.

The original multi-namespace addendum's forwarding argument alone was
insufficient to establish reachability: ordinary initial namespace completions
are combined into one `InitDone`, and a post-completion `Init` exits the
controller before its following completion. However, independent review found
a concrete different interleaving: A completes, A starts a new LIST while B is
still initializing, B completes, and then A finishes its new LIST. The merger
suppresses A's second Init without clearing its completed flag, so it can emit
two global completions. This is a source-level state-machine finding, not a
live-cluster reproduction. The original claim of unconditional dormancy is
therefore too strong.

That old interleaving also permitted direct `InitApply` processing after worker
startup and a partial reflector-store replacement before the duplicate reaches
this guard. The source-snapshot correction linked above addresses those separate
lifecycle effects; the original small guard only prevented the second worker
and repeated completion callbacks. FS initial
dispatch and etcd resync do not demonstrate an ordinary duplicate: etcd
compaction recovery emits runtime `Apply`/`Delete` events. The missing guard
was a real invariant violation, not an accepted tradeoff; no production incident
is claimed here.

## Alternatives and limits

Guard the entire completion branch rather than only assigning a handle: the
handler initialization callback and ready notifications must not repeat either.
Do not restart a worker merely because another completion arrives, and do not
infer that the historical small guard alone established source relist safety.

This correction does not make arbitrary malformed `InitApply` sequences safe,
supervise worker panics, or change existing shutdown timeout behavior. It does
not establish safety for every possible interleaving of namespace relists.
Inline status IO remains the accepted serial-worker design documented in
`../../handler-requeue/intentional-design/worker-path-status-io-inline-accepted.md`.

## Verification and re-evaluation

`file_system::resource_controller::tests::duplicate_init_done_keeps_one_worker`
drives the real event loop through first completion, duplicate completion, and
a repeated empty `Init`/`InitDone` sequence. It checks processor ownership by
exactly one worker, a single handler completion callback, and release of the
worker reference after shutdown. Events are drained by explicit future polling;
the test needs no server or timing sleeps.

The historical Kubernetes
`namespace_watchers_expose_one_union_initialization_sequence` test covers
ordinary initial namespace merging. Kubernetes and etcd completion guards are
also checked by source review; the FS regression is not an end-to-end test of
those backends. No dynamic integration scenario can inject this internal FS
completion event through the normal configuration CRUD API.

Historical verification for the original small guard (not rerun by editing this page):

- Before the guard, the new FS worker-ownership assertion failed: four processor
  owners instead of three after a duplicate completion.
- `cargo test -p edgion-controller --lib conf_mgr::conf_center::`: 161 passed,
  6 existing tests ignored, 0 failed. Includes the FS duplicate-completion and
  Kubernetes union-initialization tests above.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, agent-doc
  validation, SSA force, metrics inventory, and Pingora source policy passed.
- `cargo clippy --workspace --all-targets` passed with existing warnings.
- Independent read-only review returned LGTM after reviewing all three guards,
  the regression test, this knowledge page, and the separate namespace relist todo.

Re-evaluate when watcher event generation/merging, initialization callback
semantics, worker ownership, or controller restart behavior changes.

## Resolution references

- `initdone-double-spawn-second-worker` (filed 2026-07-17): real defensive
  correctness defect; small fix approved by the user and verified. Closed as
  fixed on 2026-09-07. The later source-snapshot correction has separate
  acceptance evidence in the linked regression guard.
