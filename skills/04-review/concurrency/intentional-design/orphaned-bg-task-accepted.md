---
name: orphaned-bg-task-accepted
description: Use when reviewing `gateway/conf_sync/cache_client/cache_data.rs::set_conf_processor` "JoinHandle dropped / no shutdown abort / no double-call guard / handler==None branch is dead code" findings. Reject re-reports that propose storing the JoinHandle for abort, adding a `debug_assert!`/`assert!` single-call guard, or rewriting the dead-code termination branch as `unreachable!()`.
knowledge_type: intentional-design
---

# `CacheData::set_conf_processor` background debounce task — `JoinHandle` intentionally discarded, no double-call guard

**Status:** ⚠️ accepted tradeoff · 2026-05-28
**Severity:** Low (BadLogic — latent; current reachability zero)

## Conclusion

`CacheData::set_conf_processor`
(`edgion-gateway/src/conf_sync/cache_client/cache_data.rs:101-124`) spawns a `tokio::spawn`
debounce task and drops the returned `JoinHandle`. The task loops on `notify.notified().await`
and only exits via the `handler.as_mut()` `None` branch in `process_compressed_events`
(lines 135-144), which is unreachable under current code because `self.handler` is only ever
written (in `set_conf_processor` itself) and never cleared. This stays as-is.

Fix suggestions of the form "store the `JoinHandle` in `ConfHandlerData`/`CacheData` and
`abort()` on a second `set_conf_processor` call or on shutdown", "add
`debug_assert!(self.handler.is_none(), ...)` at the top of `set_conf_processor` to enforce the
single-call invariant", or "replace the dead `handler == None` `tracing::error!; return false`
branch with `unreachable!()`" are **not accepted**.

## Core Rationale

**1. The orphan scenario has zero reachability today.**

`set_conf_processor` is called exactly once per `ClientCache` in `ConfigClient::new`
(`edgion-gateway/src/conf_sync/conf_client/config_client.rs:52-153`) — one site per resource kind,
each on a freshly-constructed cache, no public API exposes a second call. There is no live bug:
no orphan task can be produced, no `Arc<RwLock<CacheData<T>>>` is held against release, no Tokio
worker slot is leaked. The finding is a latent-misuse hazard, not a defect.

**2. The "long-lived task, no abort handle" choice is the documented project policy for
control-plane singletons.**

`ConfigClient` (and every `ClientCache<T>` inside it) is constructed once at gateway startup and
lives for the entire process lifetime. The debounce task is structurally identical to the other
"start at boot, die with the process" tasks in the data plane; no test runtime ever instantiates
a real `ConfigClient`, so the "tests can't clean up the task" framing does not bite either —
unit coverage in `cache_data.rs` deliberately skips the `set_conf_processor` path (see the test
module's `//!` header citing `tasks/todo-test-cover/05-integration-gaps-controller.md`). The
sibling `link_sys` providers that *do* store a `JoinHandle` and `abort()` it (see
[`etcd-monitor-handle-not-aborted.md`](../regression-guards/etcd-monitor-handle-not-aborted.md))
are reconnect-loop-with-shutdown-signal, not a fire-and-forget debounce; they need abort for a
concrete write-lock-contention reason on shutdown that the conf-sync debounce path does not have
(no consumer waits to acquire a write lock against the task on shutdown — process exit just drops
the runtime).

**3. The proposed `debug_assert!`/`assert!` single-call guard buys ~zero protection at the cost
of bloating the doc surface.**

Adding `debug_assert!(self.handler.is_none(), "set_conf_processor called twice")` would only
fire in `cfg(debug_assertions)` builds; release builds (the only builds that matter for the
"orphan a task" scenario) are unchanged. No `cargo test` invocation exercises the path (above
test-module note), so the assert is dead in test runs too. An always-on `assert!` is worse —
the panic would have to be paired with a doc-comment block explaining why `set_conf_processor`
panics on a second call when nothing in the public API documents that contract, and the
finding's actual concern (a future refactor unwittingly re-using a cache) is better caught by
the natural code review that introduces the second caller than by a runtime panic at gateway
startup. The single-call invariant is already enforced socially by the architecture: all
`ClientCache<T>` constructors are private to `ConfigClient::new`, and every call site is
co-located in the same constructor.

**4. The dead `handler == None` branch is defensive coding, not a defect.**

The `process_compressed_events` (`cache_data.rs:135-144, 201-209`) `None` arms log
`"Handler was removed..."` + `return false`. Under current code `handler` is never cleared, so
both arms are unreachable. Rewriting them as `unreachable!()` would turn a benign "spin
silently if architecture changes" into "panic on the data path if a future contributor adds
handler clearing for a legitimate reason (e.g. a hot-reload that swaps the processor)". The
`return false` + `tracing::error!` shape is the project idiom for "this should not happen, log
and bail rather than panic"; same class as
[`link-sys-F014`](webhook-active-health-abort-only.md)'s decision to keep an
abort-only loop rather than insist on a never-trip exit assertion.

## Fix Suggestions Not Accepted

- "Store the `JoinHandle` in `ConfHandlerData`/`CacheData` and `abort()` it on shutdown or on a
  second `set_conf_processor` call" — adds a `parking_lot::Mutex<Option<JoinHandle<()>>>` field
  + `Send` bound review + shutdown-ordering review for a scenario that does not exist; the
  long-lived-singleton policy already accepts "task dies with the process", same as the rest of
  the control plane.
- "Add `debug_assert!`/`assert!(self.handler.is_none(), ...)` at the top of
  `set_conf_processor`" — `debug_assert!` does not run in release; `assert!` adds a runtime
  panic surface without doc-API support and protects against a misuse that the constructor
  geometry already prevents.
- "Rewrite the `handler == None` branch as `unreachable!()`" — promotes defensive logging into a
  data-path panic; future legitimate handler-clearing refactors would have to revert this first.
- "Add a comment-only note explaining the unreachable branch" — partially defensible, but the
  doc-comment on `set_conf_processor` would balloon to cover four contingencies that no caller
  needs to reason about (current single-call invariant, why no abort, why dead branch retained,
  what to change first if any of the above invariants change). This
  rule file is the canonical place for that reasoning; the source stays minimal.

## Re-evaluation Triggers

Re-open this decision only if:

- A second `set_conf_processor` call site appears (e.g. hot-reload of a `ClientCache`, a new
  test-only constructor that wires up a processor, a federation feature that reseats handlers
  at runtime). The single-call invariant then no longer holds and the abort path becomes a
  real requirement, not a paranoia hedge.
- `ConfigClient` gains a graceful-shutdown phase that waits for in-flight `partial_update`
  invocations (today it has none — process exit drops the runtime). Then the
  store-and-abort-handle policy from `link-sys-F012` should be adopted.
- A test harness lands that instantiates `ConfigClient` against an in-process fake gRPC server
  and needs deterministic task teardown between cases (today such tests are deferred per
  `tasks/todo-test-cover/05-integration-gaps-controller.md` #3/#4).

## Reference Cases

- conf-sync-F006 (review finding, `tasks/conf/`, 2026-05-28)
- [`etcd-monitor-handle-not-aborted.md`](../regression-guards/etcd-monitor-handle-not-aborted.md)
  — the sibling case where store-and-abort *is* required; explains the write-lock-contention
  trigger that does not apply to conf-sync's debounce task.
- [`webhook-active-health-abort-only.md`](webhook-active-health-abort-only.md)
  — sibling abort-only monitor decision; same "long-lived task, no drainable state" framing.
- [`partial-update-under-lock-intentional.md`](partial-update-under-lock-intentional.md)
  — companion ⚠️ for the same `process_compressed_events` body (write-lock-held-across-
  `partial_update`); both rules together document the conf-sync debounce path's locking +
  task-lifecycle decisions.
- `edgion-gateway/src/conf_sync/conf_client/config_client.rs:52-153` — the constructor that
  encodes the single-call invariant by construction.
- `edgion-gateway/src/conf_sync/cache_client/cache_data.rs:270-291` — the test module's `//!`
  header documenting why `set_conf_processor` is not unit-tested.

## Resolution references

- `conf-sync-F006` — 2026-05-28 (⚠️ accepted tradeoff).
