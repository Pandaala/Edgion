# H2 Audit Issues

These are independent remediation issues discovered during the whole-change H2
audit on 2026-08-26. All production implementation remains blocked until the
relevant security issues are fixed, tested, and independently reviewed.

| ID | Severity | Scope |
|---|---|---|
| H2-001 | P2 closed 2026-09-01 | Accepted upstream h2 trailer-decoder limitation; wait for upstream while retaining Pingora safety guards; rationale in `skills/04-review/h2-grpc/regression-guards/h2-trailer-decoder-upstream-limitation.md` |
| H2-002 | P0 closed 2026-09-09 | Read-error and incomplete-EOF poisoning already fixed; revalidated in `skills/04-review/h2-grpc/regression-guards/h2-end-stream-wire-observer.md` |
| H2-003 | P1 | Terminal HEADERS state must not depend on preceding DATA |
| H2-004 | P1 deferred | Direct and poll trailer APIs require a permanent terminal-error latch; blocked by the accepted upstream h2 trailer-decoder limitation |
| H2-005 | P1 closed 2026-08-27 | GOAWAY must have persistent eligibility and poison state; fixed in pingora fork `edgion_v3`, rationale in `skills/04-review/h2-grpc/h2-goaway-persistent-ceiling-fail-closed.md` |
| H2-006 | P1 | Local reset must irreversibly invalidate shared observation before reset |
| H2-007 | P1 closed 2026-08-27 | A request-body write blocked on upstream flow control must be bounded once the peer flagged END_STREAM, and the bounded outcome must deliver the response; fixed in pingora fork `edgion_v3`, rationale in `skills/04-review/h2-grpc/regression-guards/h2-writer-capacity-stall-after-response.md` |
| H2-008 | P1 closed 2026-08-27 | Shutdown H2 connections must not allocate or be reused; fixed in pingora fork `edgion_v3`, rationale in `skills/04-review/h2-grpc/regression-guards/h2-shutdown-connection-not-allocatable.md` |
| H2-009 | P1 closed 2026-08-28 | CI must enforce security contracts and dependency coverage; closed `wont-fix` at the review layer because it is a CI/CD change in the pingora fork repository, rationale in `skills/04-review/h2-grpc/regression-guards/h2-ci-contract-enforcement-out-of-scope.md` |
| H2-010 | P2 | Tests must prove the staged poll-error latch and raw-wire ordering |
| H2-011 | P1 closed 2026-08-28 | Baseline tests must not require known h2 vulnerabilities; fixed in pingora fork `edgion_v3`, rationale in `skills/04-review/h2-grpc/regression-guards/h2-dependency-baseline-tests-version-robust.md` |
| H2-012 | P1 closed 2026-08-28 | Proxy cache/reuse test harness requires end-to-end consumers; fixed in pingora fork `edgion_v3` by the new `test_h2_upstream_cache_and_reuse` target, rationale in `skills/04-review/h2-grpc/regression-guards/h2-proxy-cache-reuse-harness-consumers.md` |
| H2-013 | P2 | Fork H2 documentation must match executable evidence |
| H2-014 | P2 | Per-failure-case connection disposition policy; split out of H2-008 |

Do not close an issue merely because an ignored contract exists. The contract
must be enabled, pass at the declared h2 minimum and current resolution, and
receive independent review.
