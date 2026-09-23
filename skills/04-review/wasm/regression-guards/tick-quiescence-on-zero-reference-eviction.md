---
name: wasm-plugin-owner-background-shutdown
description: Review Wasm owner destruction, late publication, tick cancellation, and request lease survival.
knowledge_type: regression-guards
---

# Wasm plugin destruction closes its background work

## Conclusion and scope

The configured plugin owns a non-Clone `WasmRuntime` and its `PluginTaskOwner`.
Background preload/refresh tasks may retain shared runtime state, and a tick may
retain a module entry for one round, but neither owns the plugin/runtime owner.
The last common plugin Arc therefore triggers destruction even while background
work is awaiting a response.

Destruction closes the task scope, aborts preload/refresh, disables and aborts the
published tick, and releases publication. Task creation and publication use the
same short active-scope gate as closure: work completing after closure cannot
publish or start a new tick. No gate is held across an await. Fixed task slots
bound retained task handles rather than accumulating a history of completed tasks.

Tick startup rechecks enablement while holding its task mutex; shutdown disables
it and takes its handle under that same mutex. An old request can keep using its
entry, but acquiring another VM cannot restart the disabled background task.
Successful OCI refresh stops the old tick and publishes the replacement while the
owner is active. Failed refresh retains the last successful entry.

## Cancellation and limits

Shutdown requests synchronous abort; it does not wait in Drop. Abort lands at a
host await point; a synchronous guest call remains bounded by the engine epoch
deadline. Brief overlap between retiring and replacement generations is permitted
by the approved design. There is no rollback or zero-overlap handoff promise.

A tick lease is poisoned during callout resolution and restored only after a
clean round. Cancelling a pending callout therefore discards the VM rather than
pooling partial guest state. An aborted response is not delivered to the guest.
Ordinary request leases retain their own entry until they finish.

The ownership decision is recorded in
[plugin runtime isolation](wasm-module-cache-key-runtime-config.md).
It does not add new observability, alter request authorization, or weaken outbound
call caps and egress checks.

## Historical correction

`wasm-tick-retention-exceeds-residue-deadline` (2026-09-23) established that
removing a global cache entry and relying on `ModuleEntry::drop` was insufficient:
a tick could hold the entry across roughly 80 seconds of serial callouts. The old
fix explicitly awaited tick shutdown after the 60-second eviction grace. Its
64-test Wasm run was historical verification of that manager implementation.

The plugin-owned design removes both the grace and the global eviction mechanism.
It preserves the underlying correction: owner retirement explicitly cancels work;
the tick's own strong entry reference cannot postpone the cancellation signal.
The earlier knowledge claim that OCI refresh could publish into a removed slot
was already stale before this redesign: current-at-review refresh code rechecked
slot identity after awaiting stop and held its map guard through publication.

## Verification and re-evaluation

The 2026-09-23 ownership run passed all 226 selected Gateway Wasm tests and the
generic task-scope test. Coverage includes owner destruction, rejected late publication,
disabled entries refusing tick restart, request lease survival, and cancellation
discarding poisoned VMs. Historical grace/zero-reference tests were replaced
with these ownership boundaries without compatibility APIs.
The same run passed the dynamic ownership scenario, all nine static Wasm
integration tests, and the final cache-residue gate after resource deletion.

Re-evaluate if tasks gain owner references, publication bypasses the active gate,
guest execution loses epoch bounds, or cancellation ceases to discard dirty VMs.

## Resolution references

- `wasm-tick-retention-exceeds-residue-deadline`: explicit background cancellation
  remains necessary; its former awaited global eviction implementation is historical.
- `wasm-tick-callout-grant-eviction-race`: plugin-owned runtime replacement.
