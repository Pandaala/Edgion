# B0212 — UDP Proxy

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/edgion_udp.rs`.
- Baseline: `f48fca38`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0212 analyst.
- Investigate session/socket and route ownership, bounded lifecycle, plugins,
  endpoint safety and canonical traffic observability through actual callers.
- No implementation authorized; existing integration files and user-owned paths remain protected.

## Analysis Outcome

- Decision: change; simple. Target blob `0630ba1e763ce2e39280c6cfe9e66be538fbe3ee` at `f48fca38`; no tests run during analysis.
- ClientSession already has shared Arc ownership; its activity mutex and four counters have no independent Arc consumers. Inline them without changing locking/Ordering, removing five per-session allocations.
- Move canonical IP formatting off successful established-session sends into the existing error/slow paths; retain canonical output and logging behavior. No measured RSS/throughput claim.
- Exact proposed scope: target and `skills/04-review/cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md`.
- Preserve session pinning, 60-second inactivity, 10,000 admission cap, 64 KiB buffers, socket/permit/task/retirement behavior and family-matched bind. Run UDP and existing UDP log tests; do not introduce real-socket tests or rewrite shared session mechanisms.
- D0120 separately records identity/activity retirement races; D0121 records discarded upstream sender identity. Analyst searched original knowledge and tasks, found no duplicate mechanism issue, and confirms local allocation changes can safely proceed without changing or claiming to fix these residuals. Both later fixes require reviewed plans.
