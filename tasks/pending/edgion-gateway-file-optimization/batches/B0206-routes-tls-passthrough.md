# B0206 — TLS Passthrough Proxy

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/passthrough_proxy.rs`.
- Baseline: `b29c0701`.
- Analyst: `b0206_tls_passthrough_analysis`.
- Investigate ClientHello/SNI trust, routing, backend dial/retry/timeout,
  connection ownership and canonical logging/accounting across actual callers.
- D0112, D0114 and D0115 remain separate cross-cutting issues. Existing
  integration files and user-owned paths are protected; no implementation yet.

## Analysis Outcome

- Decision: change; simple. Target blob `ebc24857edfe3ae22b130881ea281b4b9fe72079` at `b29c0701`; no tests run during analysis.
- Scope route-table load/match synchronously so only the selected unit Arc survives connection awaits. The matcher already returns an owned Arc.
- The caller owns a 16 KiB heap Vec and lends it across the entire relay. Truncate to filled bytes without reallocating, transfer ownership to setup, then explicitly drop after initial write/flush/accounting and before duplex. This guarantees a heap-release boundary, not a measured RSS or future-frame-size improvement.
- Borrow the endpoint address instead of cloning while its backend Arc remains alive; preserve bracket-safe string, validation and PP2 behavior.
- Exact proposed scope: target; new routing guard `tls-passthrough-connection-ownership.md`; routing index; existing TLS guard `sni-passthrough-clienthello-read.md`.
- Verify production selection via Weak old-table/unselected release and selected generation retention; in-memory initial filled-byte replay including trailing early data, write/flush error termination and accounting. Do not add allocator instrumentation or shared helper changes solely for tests.
- D0116 tracks confirmed medium-severity TLS log masking gaps; D0117 separately investigates setup deadline coverage without claiming proven unbounded hangs. Current ownership work may safely continue without changing either behavior.
