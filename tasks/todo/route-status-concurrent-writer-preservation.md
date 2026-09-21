# Preserve Concurrent Route Status Writers

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-21 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Prevent Edgion's Kubernetes status persistence from losing a concurrent foreign
controller update to a Gateway API Route's `status.parents`. The repair belongs
at the storage boundary and must preserve the existing canonical parent identity,
configured-controller ownership, serial workqueue and inline status-I/O model,
and Controller-to-Gateway publication ordering.

## Evidence

Route handlers preserve foreign `status.parents` entries from the resource
snapshot they process and update only Edgion-owned canonical entries. Kubernetes
persistence currently sends the resulting whole status with JSON Merge Patch.
Because `parents` is an array, the patch replaces that array. A foreign controller
write that lands after Edgion's input snapshot but before Edgion's status patch can
therefore be overwritten even though the handler-level ownership logic is correct.

This limitation applies to HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, and UDPRoute.
It is distinct from canonical identity and ownership selection, which are covered
by [Canonical parent status ownership](../../skills/04-review/status/regression-guards/canonical-parent-status-ownership.md).

## Requirements

- Investigate and implement storage-level concurrent-writer preservation for
  Route `status.parents` updates in Kubernetes mode.
- Preserve foreign-controller entries from the current server state, including
  changes committed after the worker's original input snapshot.
- Apply Edgion's intended owned-entry replacement and stale/duplicate owned-entry
  pruning using the existing canonical `ParentReference` identity plus configured
  `controllerName`; do not broaden, weaken, or duplicate that identity logic.
- Detect and safely retry a race that occurs while reconciling or persisting the
  refreshed status. A retry must converge from a fresh server state and must not
  silently degrade to an unconditional whole-array overwrite.
- Carry the worker input object's UID and generation into persistence. Re-read,
  re-merge, and retry only while both still match that input. If the Route's spec
  generation changes, or deletion/recreation changes its UID, reject the stale
  status write and let normal queued reconciliation process the new object.
- Preserve the accepted
  [status-write failure policy](../../skills/04-review/handler-requeue/intentional-design/status-write-no-retry-accepted.md):
  a bounded inline retry is permitted only for the new optimistic-concurrency
  conflict after identity and generation checks. Do not retry 403, 422, 429, 503,
  transport failures, or other status-write errors; do not requeue the key or
  create a background retry path.
- Keep status persistence inline in the resource kind's sole serial worker. Do not
  add an asynchronous status writer, another queue, a background retry task, a
  timer, or parallel per-kind mutation.
- Preserve the existing processing and publication barrier: status persistence
  must not cause stale configuration publication, reorder the accepted generation,
  or create a second authority for Route acceptance.
- Preserve transition-time behavior and the existing Gateway consumption rule:
  only the configured controller's canonical entry may determine Edgion Route
  attachment; foreign status remains retained data, not acceptance evidence.
- Keep non-Kubernetes storage behavior unchanged unless shared code requires a
  narrowly justified adjustment.

## Scope

### In scope

- The Kubernetes status persistence path and the minimum Route-specific merge or
  compare-and-swap support needed for the five Gateway API Route kinds.
- Focused deterministic tests for interleaved Edgion and foreign-controller status
  writes within one UID and generation, optimistic-concurrency conflict retry,
  canonical owned-entry replacement, and foreign-entry retention.
- Deterministic stale-input tests in which the Route's generation changes or the
  object is deleted and recreated under a new UID between worker input and status
  persistence.
- Revalidation of existing parent identity, status diff, worker serialization,
  publication, and schema/transport regression coverage.

### Out of scope

- Changing canonical parent identity, configured controller ownership, or Gateway
  Route acceptance semantics.
- Converting all status writers or all Kubernetes patches to a new persistence
  mechanism without evidence that this Route-specific repair requires it.
- Moving status I/O outside the worker or relaxing the single-workqueue invariant.
- Adding new CRD fields, configuration knobs, metrics, logs, or integration-test
  topology solely for this repair.

## Acceptance criteria

- A deterministic race test proves that a foreign `status.parents` update made
  after Edgion's input snapshot survives Edgion's completed status write.
- A second write during Edgion's refreshed read/write window is detected and
  retried or rejected without data loss; retry behavior is bounded and returns an
  actionable error when convergence cannot be obtained.
- Same-generation foreign-writer conflicts exercise the bounded inline re-read,
  re-merge, and conditional-write path. A generation change or UID change rejects
  the stale attempt without writing status for the new object; its ordinary watch
  or relist event remains responsible for reconciliation.
- Tests prove that only the designated optimistic-concurrency conflict is retried.
  Authorization, validation, throttling, server, and transport failures preserve
  the existing warn-and-wait-for-a-later-reconciliation-opportunity behavior, with
  no immediate requeue or background retry.
- Edgion can still replace its canonical owned entry, prune stale or duplicate
  owned entries, and preserve distinct foreign entries across all five Route kinds.
- Empty or absent `parentRefs`, foreign-only status, and malformed or duplicate
  status inputs retain their established semantics.
- The sole per-kind worker remains the inline owner of status I/O, and existing
  configuration publication ordering remains unchanged.
- Focused Route status tests, relevant resource-processor and publication tests,
  formatting, workspace check and Clippy, and repository documentation guards pass.

## Investigation notes

Evaluate Kubernetes-native optimistic concurrency mechanisms at the status
subresource boundary, including a fresh read followed by a resource-version-guarded
write or an equivalent atomic conditional patch. Confirm the selected mechanism's
actual API-server semantics for custom resources and arrays before implementation;
do not assume that JSON Merge Patch, strategic merge behavior, or field ownership
protects individual `parents` elements. Treat resource version as the conditional
write token, not as proof that the worker still targets the same desired object:
UID and generation must independently match the worker input before each merge or
retry.
