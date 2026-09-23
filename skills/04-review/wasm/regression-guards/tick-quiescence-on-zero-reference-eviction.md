---
name: tick-quiescence-on-zero-reference-eviction
description: Use when reviewing Wasm module eviction, tick task lifetime, tick callout cancellation, or cache-residue deadlines.
knowledge_type: regression-guards
---

# Zero-reference eviction explicitly quiesces the module tick task

## Conclusion and scope

`WasmVmManager::unregister_plugin` owns the quiescence boundary for a module's
background tick task. When the plugin refcount stays zero through the
`EVICT_GRACE_SECS` (60 s) anti-thrash grace, the eviction task removes the cache
entry and then calls `ModuleEntry::stop_tick().await` on the evicted entry. Map
removal alone is not a stop signal, and `Drop`'s abort is only a teardown
fallback. A tick round parked in a callout is therefore cancelled at eviction
instead of running to its own callout budget.

Cancellation lands on a host await point, so guest execution is never cut in the
middle of a call; a synchronous guest call in progress is bounded by the engine
epoch deadline. The tick round poisons its `VmLease` for the duration of callout
resolution and restores health only after a clean round, so a cancelled round
discards its VM rather than pooling one with a taken pending-callout queue or a
staged callout response.

Scope: the module-level tick task only. Request-owned `VmLease`s are untouched;
they keep the entry alive until they return, exactly as before.

## Rationale and evidence

A tick round calls `weak.upgrade()` and holds the resulting strong
`Arc<ModuleEntry>` across `resolve_callouts_on(...).await`. The previous design
relied on `Drop for ModuleEntry` to abort the task, which requires the strong
count to reach zero — a state the running round itself prevents. The stop
mechanism could only act on a task that had already stopped. With 8 dispatches
per tick (`MAX_CALLOUTS_PER_REQUEST`), a `calloutTimeoutMs` up to 10 000 ms, and
serial per-callout deadlines in `callout.rs`, one round could retain the entry,
its `SharedData`, and its VM pool for roughly 80 s past the 60 s eviction point,
while the guest kept issuing outbound callouts for an already-deleted plugin.

The fix reuses the shape the OCI refresh handoff already relies on
(`refresh_oci_entry` → `old_entry.stop_tick().await`), whose comment already
records that `abort` without awaiting termination is insufficient. The eviction
closure now returns the evicted `Arc<ModuleEntry>` so the spawned grace task can
await that shutdown; the no-runtime teardown path cannot await and keeps the
`Drop` fallback.

Authority: `skills/00-design-rules.md` (resource lifetime and failure policy) and
the documented contract in
`docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md` — "The tick task
stops automatically when the module's cache entry is evicted" — which the code
now actually honors.

## Alternatives and limits

Cooperative cancellation (a flag checked between callouts) was rejected: it still
waits out the in-flight callout's full deadline, adds a second lifecycle signal
next to `tick_enabled`, and buys nothing, because abort cannot interrupt guest
code anyway. Raising the residue deadline instead of stopping the task was
rejected: it would encode an unbounded production behavior as a test constant.

Limits: no lock is held across the shutdown await (`stop_tick` takes the join
handle out of its mutex first), and the evicting task is never the tick task, so
self-abort cannot occur. The narrow pre-existing race where a concurrent
`acquire` re-arms a task between `tick_enabled = false` and the handle swap is
unchanged by this fix — it is the same window the refresh handoff accepts. A
refresh task that republishes into an already-removed slot remains a separate
pre-existing concern: that orphaned slot keeps a strong reference and a live tick
task, which `Drop`'s abort could never reach either. When a refresh handoff and an
eviction call `stop_tick` concurrently on the same entry, the loser takes `None`
out of the mutex and returns without awaiting; quiescence still happens because
the winner awaits it, but the evicting task's own await no longer proves it.
Cancelling mid-callout drops that callout's result without
delivering it to the guest and emits no delivery metric for it; the discarded VM
is counted as `edgion_wasm_instance_discard_total{reason="poisoned"}`.

## Verification and re-evaluation

Inline `vm_manager::tests`:

- `zero_reference_eviction_stops_a_tick_round_blocked_in_a_callout` drives a real
  tick callout against a local listener that accepts and never answers (30 s
  callout budget) and asserts the tick task count reaches zero within 5 s of
  unregistration. Confirmed to fail (timeout waiting for 0 tick tasks) when the
  `stop_tick().await` call is removed, so it is a true regression guard.
- `reregister_during_grace_keeps_the_tick_task_running` asserts that a
  re-registration inside the grace preserves the entry, `tick_enabled`, the join
  handle, and the single live tick task.

`cargo test -p edgion-gateway --lib wasm::vm_manager` passed 64 tests, including
the existing eviction, refresh-handoff, and tick-callout panic tests.

The production quiescence bound is now the 60 s grace plus a bounded shutdown
(one host await point, or one epoch-bounded guest call). The local runner
(`run_integration.sh --timeout-secs 75`), the Kubernetes gate
(`run_cache_residue_gate.sh`, `VALIDATOR_TIMEOUT_SECS=75`), and
`cache_residue::DEFAULT_TIMEOUT_SECS = 75` already agree and remain valid upper
bounds; no deadline change was needed.

Re-evaluate if `EVICT_GRACE_SECS` changes, if the tick loop gains another await
that must survive cancellation, if `VmLease::drop` stops discarding poisoned VMs,
or if module eviction moves into the consolidated lifecycle record proposed by
`tasks/todo/atomic-wasm-plugin-lifecycle-state.md`.

## Resolution references

- `wasm-tick-retention-exceeds-residue-deadline` — task created 2026-09-21,
  fixed 2026-09-23 by explicit eviction-time tick shutdown plus cancellation-safe
  tick leases. Supersedes the deferred limitation previously recorded in
  [shared-data-gauge-live-stores](shared-data-gauge-live-stores.md).
