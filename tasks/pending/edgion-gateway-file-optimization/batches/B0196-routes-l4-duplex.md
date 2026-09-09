# B0196 — L4 Duplex

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/l4_duplex.rs`.
- Analysis baseline: `387ec0d7`.
- Analyst: `b0196_duplex_analysis`.
- Investigate duplex lifecycle, cancellation, accounting, shared callers, and bounded streaming before selecting a value outcome.
- Existing `edgion-tests/**` and user-owned paths remain protected; no implementation is authorized yet.

## Analysis Outcome

- Decision: change; complex, plan-first. Target blob: `d623f5a942b8e66847c73828bbb4ad293f493828`.
- Awaiting a whole directional write/flush inside a read-select arm stops polling reverse traffic. A backend awaiting client acknowledgement before consuming more upload can deadlock both directions until idle timeout. TCP, TLS terminate, and TLS passthrough share this relay.
- Use bounded per-direction polling/state in one relay future, not spawned pumps or new shared locks. Preserve public hooks/signature, two 8 KiB buffers, read-time accounting, fixed error classifications, downstream flush, immediate EOF termination, and the single idle clock reset by either direction's progress.
- Exact proposed technical scope: target source; `skills/04-review/routing/regression-guards/l4-duplex-relay-centralization.md`; `skills/04-review/security/regression-guards/l4-relay-idle-timeout.md`. Update the existing owning knowledge rather than adding parallel guidance.
- Require an in-memory old-code-failing regression, symmetric backpressure and pending-flush progress, exact bytes, shared idle/no-timeout/partial-write coverage, errors/write-zero, EOF and accounting. Preserve or explicitly replace existing helper regression coverage.
- New integration coverage for the three transports must be deferred; existing suites stay frozen. No tests were run during analysis.
