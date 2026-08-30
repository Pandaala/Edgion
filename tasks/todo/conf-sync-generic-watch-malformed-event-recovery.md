# Conf-Sync Generic Watch Malformed Event Recovery

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

The Gateway's generic conf-sync watch path can skip a malformed event after the
corresponding response version has already been consumed. Later watch events then
continue from a newer version, so the omitted change may never be replayed and the
local cache can permanently diverge from the Controller. Do not fix this with a
local parse-error branch. Design the batch failure policy, atomic application,
readiness transition, relist behavior, and compressed-event interaction together.

## Confirmed findings

- Malformed watch batch JSON is logged and the same stream continues.
- A malformed event envelope is silently skipped.
- A typed resource decode failure is logged and skipped.
- Valid events preceding a malformed event in the same batch can already mutate
  the cache before the malformed tail is observed.
- Because the response version advances independently of those skipped mutations,
  the Controller is not required to replay the missing state.
- `EdgionBackend` already has a stricter raw-identity and fail-close path, but its
  policy must not be copied to every resource kind without an explicit decision.

## Requirements

- Define one documented failure policy for malformed generic watch payloads.
- Parse and validate a complete response batch before publishing any mutation, or
  prove an equivalent atomic rollback mechanism.
- On an unsafe batch, transition to a relist generation without accepting newer
  watch versions from the invalid stream.
- Specify whether readiness is withdrawn during recovery and ensure consumers
  cannot observe a falsely complete cache.
- Reconcile pending compressed events with reset/relist so stale partial updates
  cannot run after the replacement snapshot.
- Preserve server-generation cancellation and retry-backoff behavior.
- Keep resource data and secret-bearing payloads out of logs.
- Preserve the stricter malformed-resource withdrawal semantics for
  `EdgionBackend` unless the new unified policy deliberately supersedes them.

## Acceptance tests

- A malformed envelope after a valid event in one response publishes neither
  event and forces a fresh list before watch resumes.
- A typed decode failure does not advance the durable watch position or leave a
  valid prefix applied.
- A malformed top-level batch cannot be followed by newer watch mutations before
  relist completes.
- Readiness reflects the selected recovery policy throughout the transition.
- Pending compressed events from the abandoned generation cannot execute after
  reset.
- Backend exact-key withdrawal and unsafe-batch behavior remain covered.
- Full cache-client tests and the normal workspace guards pass.

## Scope

### In scope

- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`
- `edgion-gateway/src/conf_sync/cache_client/cache_data.rs` when required for an
  atomic batch or recovery transition
- Focused conf-sync unit tests

### Out of scope

- Controller workqueue changes
- Conf-sync wire-schema changes without a separate transport design
- Broad cache implementation replacement
- Existing integration-test files

## Related context

- `skills/00-design-rules.md`
- `skills/01-architecture/03-controller-gateway-link/02-gateway-side.md`
- `AGENTS.md` conf-sync and change-discipline rules
- `tasks/pending/edgion-gateway-file-optimization/`
