# Investigate Gateway TLS Collision Snapshot Retention

## Meta

- Status: pending
- Type: performance investigation
- Priority: P3

## Evidence and Open Question

`edgion-gateway/src/runtime/matching/tls.rs` publishes complete collision
candidate vectors in its hostname and catch-all maps, including compiled serving
certificates and frontend CA stores. Lookup consumes only the first candidate.
The 64-participant warning cap bounds diagnostic output, not snapshot ownership.

This is a measurable retention opportunity, not a demonstrated security defect
or proven performance problem. Canonical Gateway configuration and old readers
also retain material; removing a candidate from the published index does not
prove all copies are released. Representative collision frequency, retained
memory and rebuild costs remain unknown.

## Investigation and Acceptance

- Revalidate both build and lookup paths. Measure single-owner and collision
  cases, separating canonical resource ownership, temporary compilation work,
  published snapshots and retained old readers.
- Compare local builder alternatives before proposing a shared `HashHost` API
  change. Require a concrete benefit that justifies complexity; no implementation
  is also a valid outcome.
- Obtain an independent implementation-plan review before runtime changes and
  freeze exact supporting paths only after ownership evidence establishes need.
- Preserve exact/wildcard/catch-all ordering, deterministic tie-breaking,
  StrictReject ownership, warning meaning and atomic publication. Verify removal
  of a winner promotes the correct runner-up on rebuild.
- Use local tests and ownership probes for old-snapshot readers and candidate
  release. Do not add production metrics/logs to measure this optimization.
- Existing integration tests remain unchanged. Record any necessary new wire
  scenario separately instead of broadening this investigation.
