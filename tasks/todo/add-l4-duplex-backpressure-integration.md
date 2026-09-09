# Add L4 Duplex Backpressure Integration Coverage

## Meta

- Status: pending
- Type: test
- Priority: P2
- Source: B0196 / D0110 in the Gateway quality campaign

## Evidence and Scope

TCP, TLS termination, and TLS passthrough share `routes/l4_duplex.rs`. Existing
integration tests cover idle closure and small periodic exchanges, but not a
backend response that must reach the client while upload writes are backpressured,
or the symmetric downstream-write/flush case.

## Acceptance Criteria

- Reuse the canonical shared Gateway/Controller topology for all three transports.
- Stall one direction while sending reverse traffic; verify progress and exact
  payload delivery without loss or duplication after the stalled side resumes.
- Verify either direction's progress keeps the shared idle timer alive and a
  fully stalled connection closes once with the existing fixed diagnostic.
- Preserve the existing immediate-EOF policy; half-close support is out of scope.
- Add the scenarios only after integration-test changes are separately scheduled.
  The campaign keeps existing `edgion-tests/**` files unchanged.
