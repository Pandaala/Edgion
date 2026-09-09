# B0206 — TLS Passthrough Proxy

## Scope

- Pipeline state: committed locally.
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

## Promotion and Implementation

- Analyst revalidated at `c70aa9fe`; subsequent `62e43e2f` changes only campaign/task records. Target blob and all non-table production dependencies remain unchanged.
- B0204 winner-only storage preserves the owned match API. Weak tests must use distinct hostname winners, not a collision runner-up that the table no longer owns.
- Implementer: `b0206_implementation`; authorized simple ownership changes and unit tests within the four paths above. Any broader or complex deviation requires independent plan approval before editing.

## Implementation and Verification

- Implementation complete in exactly four authorized technical paths. Production helpers bound table selection and own initial replay through flush/accounting/drop; endpoint address borrowing preserves transport formatting and validation.
- Four inline tests cover distinct-winner snapshot/unit/selector lifetimes and new generation, complete initial record plus trailing bytes under short writes, partial write failure and flush failure.
- Implementer passed fresh TLS 52/52, shared duplex 21/21, targeted format/diff and agent-doc (1,263 files) checks.
- Independent reviewer: `b0206_final_review`; LGTM, no Must Fix or Should Fix at `62e43e2f`. Target reviewed blob `504a34986014ad7c8fe83b31d25bc9cbc9d9df0e`. Personally passed TLS 52/52, duplex 21/21 and diff checks; confirmed all six target manual-let-else warnings predate this batch and new helpers/tests introduce none.
- Coordinator workspace format/check/Clippy, Gateway build, broader routes 595/595, docs, SSA, metrics, Pingora, Gateway tracing, English and 3,120-file freeze passed. Clippy retains the same 983 Gateway lib-test warnings as the preceding code batch; final reviewer checks target attribution.
- Commit: `aba8fd24`; no push.
