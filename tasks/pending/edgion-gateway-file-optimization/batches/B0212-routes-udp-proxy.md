# B0212 — UDP Proxy

## Scope

- Pipeline state: committed locally.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/edgion_udp.rs`.
- Baseline: `f48fca38`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0212 analyst.
- Investigate session/socket and route ownership, bounded lifecycle, plugins,
  endpoint safety and canonical traffic observability through actual callers.
- Implementation follows the exact two-path scope below; existing integration files and user-owned paths remain protected.

## Analysis Outcome

- Decision: change; simple. Target blob `0630ba1e763ce2e39280c6cfe9e66be538fbe3ee` at `f48fca38`; no tests run during analysis.
- ClientSession already has shared Arc ownership; its activity mutex and four counters have no independent Arc consumers. Inline them without changing locking/Ordering, removing five per-session allocations.
- Move canonical IP formatting off successful established-session sends into the existing error/slow paths; retain canonical output and logging behavior. No measured RSS/throughput claim.
- Exact proposed scope: target and `skills/04-review/cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md`.
- Preserve session pinning, 60-second inactivity, 10,000 admission cap, 64 KiB buffers, socket/permit/task/retirement behavior and family-matched bind. Run UDP and existing UDP log tests; do not introduce real-socket tests or rewrite shared session mechanisms.
- D0120 separately records identity/activity retirement races; D0121 records discarded upstream sender identity. Analyst searched original knowledge and tasks, found no duplicate mechanism issue, and confirms local allocation changes can safely proceed without changing or claiming to fix these residuals. Both later fixes require reviewed plans.

## Promotion and Implementation

- Analyst revalidated at `aec5caf5`: target unchanged; B0211 schema Arc sharing and TLS parser changes do not alter UDP session fields, log APIs or packet path. D0120–D0122 remain independent residuals.
- Result helper accepts SocketAddr and invokes canonical IP formatting only on error, matching the existing downstream helper pattern. Slow-path normalization remains unchanged.
- Implementer: `b0212_implementation`; authorized exact two paths and local fixture/test updates. Preserve inactivity semantics and all synchronization/retirement ordering; broader changes require independent plan review.

## Implementation and Verification

- Exact two-path implementation complete: source blob `4176c2dfdd29654482827880f837f58f0a013386`, owning knowledge blob `37cd40ab625b5bfb827004775100449018d336aa`.
- Inlined activity/counters and deferred established-send IP formatting; preserved all nine target tests and their runtime behavior. No new socket tests, integration changes or lifecycle-policy changes.
- Fresh implementer target 9/9, full UDP 34/34 and UDP logs 3/3 passed, along with targeted format/diff and docs (1,265 files).
- Independent reviewer: `b0212_final_review`; LGTM, no Must Fix or Should Fix at `aec5caf5`, both hashes unchanged. Personally passed UDP 34/34, logs 3/3 and diff; confirmed identical locking/Ordering/activity/error outputs and no target/new Clippy warning. D0120–D0122 remain unchanged residuals.
- Coordinator routes 605/605, workspace format/check/Clippy, Gateway build, docs (1,265 files), SSA, metrics, Pingora source, Gateway tracing, English-only, diff and 3,120-file freeze passed. Clippy retains 983 Gateway lib-test warnings. Commit: `3e28c944`; no push.
