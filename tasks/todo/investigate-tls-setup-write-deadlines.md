# Investigate TLS Setup Write Deadlines

## Meta

- Status: pending
- Type: investigation
- Priority: P3
- Severity: low

## Evidence and Acceptance Criteria

Passthrough PP2/ClientHello writes and flushes, and terminate-mode PP2 writes,
await before `bidirectional_relay` starts its idle clock. The configured peer
connection timeout covers connecting, not visibly these setup calls. The small
bounded payload normally fits socket buffers; no unbounded hang is established.

- Verify the pinned Pingora transport's default/inherited write deadlines and
  controlled conditions that can actually block setup.
- Define setup deadline ownership, explicit None behavior, errors and counters
  before introducing a timeout wrapper or changing failure policy.
- If a real gap is confirmed, test Pending write/flush with paused time and
  preserve PP2 followed by the exact full initial-byte sequence and accounting.
- Keep setup-write deadline ownership separate from
  [L4 timeout snapshot publication](../../skills/04-review/listeners/regression-guards/l4-timeout-snapshot-publication.md),
  which dynamically publishes resolved connect and relay idle policy for new
  connections without changing setup-write waiting policy.
- Defer new end-to-end scenarios and do not modify frozen integration files
  unless their exact paths are explicitly authorized. Do not change setup waiting
  policy until the transport investigation establishes a concrete gap.
