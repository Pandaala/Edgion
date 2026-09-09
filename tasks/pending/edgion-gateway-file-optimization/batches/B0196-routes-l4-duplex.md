# B0196 — L4 Duplex

## Scope

- Pipeline state: terminal.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/l4_duplex.rs`.
- Analysis baseline: `387ec0d7`.
- Freshness confirmed at `56cda803`: target, callers, timeout policy, and owning knowledge unchanged.
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
- Deferred integration task: [D0110](../deferred/D0110-l4-duplex-backpressure-integration.md).

## Implementation Plan

- Implementer: `b0196_implementation`; phase 1 only, no edits. Baseline relay tests: 3/3 passed.
- Plan reviewer: `b0196_plan_review`; Approved, no Must Fix. Preserve actual I/O/timer wakeups, self-wake only on budget yield, and include executor-driven regression coverage beyond manual polling.
1. Add deterministic duplex regressions proving reverse ACK progress while one directional output is full; first capture old-code failure.
2. Replace whole-write awaits with private bounded read/write/flush direction states retaining partial offsets and existing hooks/errors.
3. Poll both directions per sweep, alternating order with a 32-sweep budget and self-wake; preserve immediate EOF/error termination and read-time accounting.
4. Retain one pinned idle timer reset by either direction's byte progress; poll it each sweep, never reset merely on flush completion.
5. Cover symmetric backpressure, flush, exact bytes/accounting, errors/write-zero, EOF, shared idle/partial progress/None; explicitly replace the three helper tests with equivalent relay regressions.
6. Update only the two owning knowledge pages; preserve history and clarify the old fix's scope, then run focused verification.
- Stop and re-review before any need for caller/signature changes, split pumps, new shared state, dependencies, or wider test strategy.

## Implementation Evidence

- Old code failed both deterministic regressions: `blocked_download_does_not_block_upload` and `blocked_upload_does_not_block_download`, at the assertion that the reverse ACK must progress while the first write remains blocked (0/2 passed). Existing helper baseline remained 3/3.
- Approved bounded direction-state implementation is complete in the exact three-file technical scope. Final target tests: 21/21 using the default parallel runner, including paused-clock, executor-driven, and budget self-wake coverage. The original three helper tests have named equivalent or stronger relay replacements.
- Intermediate compile errors (missing AsyncRead/AsyncWrite imports and ambiguous test shutdown methods) were corrected within scope. Independent `b0196_compile_diagnosis` confirmed batch-caused, safe correction, no plan expansion; all later test runs passed.

## Independent Final Review

- Reviewer: `b0196_final_review`; final LGTM, no remaining Must Fix or Should Fix. The test-only Mutex warning was corrected using parking_lot and independently re-reviewed. Production behavior, all callers, knowledge conclusions and independent 21/21 tests passed review; corrected target tests also passed 21/21.

## Verification

- Target 21/21; workspace format, agent-doc, SSA, metrics, Pingora, Gateway tracing, English-only, diff and frozen-path checks passed.
- Final locked workspace check/Clippy passed; no warning references the target file after correction. All Gateway route unit tests passed 580/580. Gateway binary build passed before the test-only Mutex correction; production source is unchanged since that build.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `21ca0728`.
