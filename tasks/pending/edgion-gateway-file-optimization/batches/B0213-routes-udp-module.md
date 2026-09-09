# B0213 — UDP Module

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/mod.rs`.
- Baseline: `cb9aaf4b`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0213 analyst.
- Investigate module visibility and exports against actual callers and any
  shared runtime logic owned by the module rather than assuming a facade.
- No implementation authorized; integration files and user-owned paths remain protected.

## Analysis Outcome

- Decision: deferred. Target blob `7d01fa18b052828cc0ce4364eb12ea67e3524d36` at `cb9aaf4b`; no tests run during analysis.
- Module also owns the cancellation-token registry. Used exports and synchronous replacement/cancel/drain operations have no demonstrated local refactor value; moving cancellation out of a cold-path lock lacks supporting contention evidence.
- Only listener registration has a production caller; cancel/cancel_all do not. UDP serve runs on the auxiliary runtime outside Pingora Service ownership, and the post-Pingora shutdown watch is not bridged to its token. Runtime teardown terminates tasks after the remaining drain interval, not registry-driven graceful completion.
- D0122 records a medium process-shutdown ownership gap requiring reviewed shutdown ordering and coordinated task exit. This is not a post-process resource leak or demand for dynamic port changes, and does not block safe local optimization.
- No existing registry-specific tests; at promotion independently review deferral and run existing UDP tests without claiming nonexistent focused coverage.
