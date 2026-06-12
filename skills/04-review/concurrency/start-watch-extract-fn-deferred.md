---
name: start-watch-extract-fn-deferred
description: Use when reviewing `gateway/conf_sync/cache_client/event_dispatch.rs::ClientCache::start_watch` "256-line spawn body with 4 nesting levels / extract `'watch_block` into `run_watch_phase` / extract per-event JSON+pre_parse+cache-mutation into `process_watch_event` / no unit tests cover the watch loop" findings. Reject re-reports that propose either of the two extractions as standalone readability fixes; document the deferral rationale and the conditions under which the work should be revisited.
---

# `ClientCache::start_watch` two remaining extraction phases deliberately deferred — DEFERRED-REFACTOR

**Status:** ⚠️ accepted tradeoff · 2026-05-28
**Severity:** Low (Readability / Testability — no correctness or performance impact)

## Conclusion

`ClientCache::start_watch`
(`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:129-388`) keeps its current
256-line, 4-level-nested shape (outer `loop` → `'watch_block:` → stream `loop` →
`for event in events`) for now. The `list_and_reset` phase has already been extracted into
its own `async fn` at lines 69-126; the remaining two structural phases — the watch setup +
stream-drive loop inside `'watch_block` (lines 234-380) and the per-event
JSON-parse + `pre_parse` + `set_sync_version` + `cache_data.write()` body (lines 311-362) —
stay inline. This stays as-is.

Fix suggestions of the form "extract `async fn run_watch_phase(...) -> WatchOutcome`",
"extract `fn process_watch_event<T>(...) -> ControlFlow<...>`", or "lift the second
`grpc_client.write()` client-guard acquisition (lines 238-248) into a shared helper to remove
its duplication with the first acquisition at lines 167-179" are **not accepted** as
standalone readability fixes. They are deferred until a behavioural change to the watch path
requires touching this region for a different reason.

## Core Rationale

**1. The cost/risk profile is medium and the harm is negligible.**

The fix is single-file but requires translating `break 'watch_block` semantics into a
`WatchOutcome` enum return type, threading the `T: ResourceMeta + Resource + Clone + Send +
'static` trait bounds through two new helpers, and verifying that the lock-acquisition and
lock-release timing of the second `grpc_client.write().await` (lines 238-248) is preserved
across the extraction. There is no unit-test coverage of the watch loop today (the test
module at lines 391-507 only exercises `apply_change` and `set_ready`; the `//!` header at
lines 392-401 defers full `start_watch` coverage to integration in
`tasks/todo-test-cover/05-integration-gaps-controller.md`). With no regression net for the
async + label-break + generic refactor, the structural-cleanup work has the same medium risk
as a behavioural change while contributing zero functional benefit and no measurable
correctness or performance improvement — current behaviour is correct at runtime.

**2. The duplication is structural-pattern, not logic, and grows out of the same async
write-lock-release discipline.**

The second client-guard acquisition at lines 238-248 looks like a copy of the first at
167-179, but its purpose is the same one applied in two different control-flow contexts: take
a brief `grpc_client.write().await`, clone the `ConfigSyncClientService<Channel>` handle out,
and drop the guard before any RPC begins (the project rule documented in
[`async-write-lock-across-rpc.md`](async-write-lock-across-rpc.md)
— never hold the async write guard across the list/watch RPC or the stream lifetime).
Extracting this into a shared helper would have to return either `Option<C>` or
`Result<C, NotInitialized>`, and the two call sites' fallback paths differ — the outer loop's
`None` arm increments `backoff_secs` and `continue`s the outer loop (lines 174-176), while
the `'watch_block` `None` arm `break 'watch_block`s back into the outer loop without touching
backoff (line 245). Threading both fallback shapes through one helper trades a structural
duplication of ~12 lines for an enum-coded control-signal that the caller must still pattern-
match — a net wash on readability and an extra layer of indirection on the hottest async path
inside this loop.

**3. `process_watch_event` extraction is the cleanest of the three but is being
intentionally bundled with the next behavioural change to this region.**

The per-event body (lines 311-362) is the most independently extractable chunk — its inputs
(`event: serde_json::Value`, `response_sv: u64`, `cache_data: &Arc<RwLock<CacheData<T>>>`)
are well-bounded and `process_watch_event` could become independently unit-testable. The
deferral is not "this extraction is bad" but "we are not paying the test-write + helper-
plumbing cost on its own". Two open issues already targeting this region —
[`conf-sync-F013-watch-event-parse-error-drops-event-silently`](../../../tasks/conf/conf-sync-F013-watch-event-parse-error-drops-event-silently.md)
(silent `continue` on `serde_json::from_value::<T>` failure at line 357, no metric / no
relist trigger) and
[`conf-sync-FB02-per-event-write-lock-churn-in-watch-loop`](../../../tasks/conf/conf-sync-FB02-per-event-write-lock-churn-in-watch-loop.md)
(one `cache_data.write()` per event vs. one per batch) — already require touching this exact
body. The current decision is: when the first of those lands, bundle the `process_watch_event`
extraction into the same change so the refactor cost is amortised against work that already
has to make this region quasi-stable. Doing it alone now would mean: refactor → write tests
→ then later behavioural-fix → re-touch the new helper → re-validate the tests against the
behaviour change. The amortised path is strictly better.

**4. The sibling already-extracted `list_and_reset` sets the precedent for "extract when the
shape is independently meaningful and stable".**

`list_and_reset` (lines 69-126) was extracted because its function-level contract —
"perform list + parse + reset cache, return `(sync_version, server_id)` or `tonic::Status`"
— is a clean async-fn shape that the outer loop simply calls. The remaining two phases do
not yet have that shape: `'watch_block`'s exit semantics carry implicit "relist needed vs.
fatal" information that is not currently distinguished (today every break path leads to the
same outer-loop relist behaviour) and would need to be invented as part of any extraction;
`process_watch_event`'s contract has open questions about how parse failures should be
surfaced (per the F013 / FB02 issues above). Extracting before those contracts are decided
locks in an interface choice that the behavioural fix will then have to revise. The project
pattern is "extract when the contract is stable", not "extract for nesting-depth alone".

## Fix Suggestions Not Accepted

- "Extract `async fn run_watch_phase(grpc_client, client_id, client_name, from_version,
  current_server_id, cache_data) -> WatchOutcome`" — invents a `WatchOutcome` enum
  prematurely; today every `'watch_block` exit triggers the same relist path, so the enum
  arms would all map to the same outer-loop action. Re-evaluate when the F013 silent-drop or
  F015 backoff-not-reset fix introduces a real branching shape.
- "Extract `fn process_watch_event<T>(event, response_sv, cache_data) -> ControlFlow<(), ()>`
  on its own" — the clean and most-defensible of the three extractions, but currently bundled
  with the next behavioural fix (F013 / FB02) so the refactor cost is amortised against work
  that already touches this body; doing it standalone forces a re-touch when the behavioural
  fix lands.
- "Lift the two `grpc_client.write().await` acquisitions into a single helper" — the two call
  sites have different fallback shapes (outer-loop `backoff_secs *= 2; continue` vs.
  `break 'watch_block`); one helper has to return a control-signal that callers must still
  pattern-match. Net wash on readability and an extra layer on the hot async path.
- "Add unit tests for the watch loop now to make the extraction safer" — the test module's
  `//!` header (lines 392-401) intentionally defers `start_watch` coverage to integration in
  `tasks/todo-test-cover/05-integration-gaps-controller.md`; the `tokio::spawn` + gRPC
  stream + lock interactions are not unit-testable without a fake gRPC server, and the
  integration scenarios (`run_conf_sync_*.sh`) already exercise relist + server-id-change +
  recoverable errors against a real controller.

## Re-evaluation Triggers

Re-open this decision only if:

- A behavioural change to the watch path lands — most likely the
  [`conf-sync-F013`](../../../tasks/conf/conf-sync-F013-watch-event-parse-error-drops-event-silently.md)
  silent-drop fix or the
  [`conf-sync-FB02`](../../../tasks/conf/conf-sync-FB02-per-event-write-lock-churn-in-watch-loop.md)
  batched-write-lock change. At that point, bundle `process_watch_event` extraction with the
  change.
- The `'watch_block` exit semantics actually diverge (e.g., a new "fatal, do not relist"
  branch is added that must propagate up to terminate the spawned task). At that point the
  `WatchOutcome` enum becomes load-bearing rather than ceremonial, and `run_watch_phase`
  becomes worth extracting.
- The kind-watcher architecture changes — for example, a single multiplexed watch stream
  replaces the 17 independent `ClientCache<T>::start_watch` spawns — invalidating the current
  shape entirely. A from-scratch rewrite would obsolete this deferral.
- The integration-test gap in
  `tasks/todo-test-cover/05-integration-gaps-controller.md` is closed and unit-level coverage
  for `process_watch_event` is identified as the remaining gap. Then the extraction-for-
  testability argument starts to pay rent.

## Reference Cases

- conf-sync-F010 (review finding, `tasks/conf/conf-sync-F010-start-watch-extract-fn-candidate.md`,
  2026-05-28).
- [`async-write-lock-across-rpc.md`](async-write-lock-across-rpc.md)
  — the async-write-lock-release discipline that both client-guard acquisitions share;
  explains why a "lift the two acquisitions into one helper" extraction is not a clean win.
- [`relist-backoff-on-server-switch-intentional.md`](relist-backoff-on-server-switch-intentional.md)
  — sibling conf-sync accepted-tradeoff on the same `start_watch` body; documents the
  precedent of keeping intentional structural shapes in this loop.
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:69-126` — the already-extracted
  `list_and_reset`, sets the "extract when the contract is stable" precedent.
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:129-388` — the current
  `start_watch` body; lines 234-380 are the `'watch_block` extraction candidate, lines
  311-362 are the `process_watch_event` extraction candidate.
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:392-401` — the test module's
  `//!` header explaining the deferred integration-test coverage that gates standalone
  unit-test motivation for the extraction.
