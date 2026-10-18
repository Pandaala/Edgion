# Request-body mutation snapshot retirement

Status: deferred design decision. Source baseline: `b71a7e522`.

## Evidence

In `edgion-gateway/src/request_body/storage.rs`, `BodyStorageView::clear`
obtains an owned snapshot and generation, checks the length, then publishes an
empty snapshot. Replacing this read with the existing `snapshot_metadata`
would avoid one Arc increment/decrement while preserving the scalar values.

The owned local has another effect: it keeps the old snapshot alive until after
`publish_arc` returns and releases its mutation write guard. `publish_arc`
currently assigns `mutation.snapshot = Some(snapshot)` while holding that guard.
For a previously mutated generation with no remaining reader, replay or scan
owner, removing clear's local pin can make that assignment destroy the last old
snapshot, including its segment vector and inline Bytes owners, inside the lock.
InitialBody still pins the original source/snapshot; it does not pin every later
mutation. This distinction prevents treating the proposed clear change as only
reference-count bookkeeping.

There is no measured contention or latency regression here, and this inspection
does not establish a production deadlock. Snapshot destruction cost depends on
segment count, retained backing and other owners. Existing byte-content,
generation, freeze and replay tests do not prove destruction occurs outside the
write guard. The candidate has not been implemented.

The earlier overwrite_body metadata change is different: its old wildcard
snapshot binding already discarded that temporary immediately. It is not
evidence that removing clear's named local preserves retirement timing.

## Decision

Keep clear's current owned snapshot, or authorize a separate publisher-level
retirement design before removing its temporary ownership. One possible design
moves the replaced snapshot into a local, releases the mutation write guard,
then drops the retired snapshot. That changes the shared publication path and
must be reviewed across all mutation callers, not folded into a two-line clear
optimization. No additional queue, background cleanup or body cache is implied.

## Acceptance for a future implementation

- Preserve incomplete-capture errors, mutation-first selection, atomic generation
  validation/increment, frozen-state checks and publication bounds.
- Preserve empty clear as a no-op, including on an already frozen empty body;
  nonempty frozen clear must still fail without changing state.
- Prove last-owner retirement occurs after the write guard is released, using a
  focused owned-Bytes drop probe that attempts a nonblocking mutation read. Also
  cover retained readers/replay and memory/disk generations.
- Verify old replay remains stable after clear and a subsequent rewind selects
  the new generation. Preserve cancellation and physical/segment limits.
- Assess critical-section and total retirement costs separately; no throughput
  benefit follows merely from eliminating an Arc clone.
- Obtain IMPL plan/final reviews and run relevant storage tests and repository
  checks before committing runtime changes.

Relevant contracts: request-body retry framing, storage I/O ownership and the
body subsystem read-contract ownership pages under `skills/04-review/http-body/`.
