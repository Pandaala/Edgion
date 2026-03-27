# Gateway API Conformance Test Failures Analysis

> Date: 2026-03-27 | Branch: feature-03-25 | Core tests: 38 PASSED / 22 FAILED

## Summary

| Group | Tests | Root Cause | Fix Effort | Priority | File |
|-------|-------|-----------|-----------|----------|------|
| GRPCRoute | 4 (23 subtests) | gRPC error response sends RST_STREAM instead of trailers; `session.shutdown()` corrupts response | Small (~20 lines) | High | [grpcroute-failures.md](grpcroute-failures.md) |
| AttachedRoutes | 4 | `update_attached_route_tracker()` counts ALL parentRefs, not just accepted ones | Small (~30 lines) | High | [attached-routes-failures.md](attached-routes-failures.md) |
| Route Status | 6 | Status persistence timing — conditions logic correct but requeue/persist pipeline breaks for status-only updates | Medium | High | [route-status-failures.md](route-status-failures.md) |
| HTTPS Listener | 2 (5 subtests) | TLS catch-all listener (no hostname) drops connection; port 8080 not in port-forward/CustomDialContext | Small (TLS) + Small (test fix) | Medium | [https-listener-failures.md](https-listener-failures.md) |
| Listener Isolation | 1 (12 subtests) | `sectionName` fast path skips hostname check in route matching | Small (~15 lines) | High | [listener-isolation-failures.md](listener-isolation-failures.md) |
| Listener Port Matching | 1 (4 subtests) | Dynamic listener creation not supported (arch limitation) | **Defer** | Low | [listener-isolation-failures.md](listener-isolation-failures.md) |
| Timeout | 2 | `"0s"` parsed as zero instead of "disable timeout"; `request_timeout` not enforced end-to-end | Medium | Medium | [timeout-rewrite-mirror-failures.md](timeout-rewrite-mirror-failures.md) |
| RewritePath | 1 | `ReplacePrefixMatch` produces `//path` (double slash) | Trivial (~3 lines) | High | [timeout-rewrite-mirror-failures.md](timeout-rewrite-mirror-failures.md) |
| PercentageMirror | 1 | `percentage` field (v1.5) not in `HTTPRequestMirrorFilter` struct | Small | Medium | [timeout-rewrite-mirror-failures.md](timeout-rewrite-mirror-failures.md) |

## Recommended Fix Order

1. **RewritePath double-slash** — trivial fix, 1 test
2. **Listener isolation hostname check** — small fix, 12 subtests across 1 test
3. **AttachedRoutes filtering** — small fix, 4 tests
4. **GRPCRoute error response** — small fix, 4 tests (23 subtests)
5. **Route status persistence** — medium fix, 6 tests (highest test count)
6. **HTTPS catch-all listener TLS** — small fix, 2 tests
7. **Timeout "0s" handling** — medium fix, 2 tests
8. **PercentageMirror percentage field** — small fix, 1 test
9. ~~Listener port matching~~ — defer (arch change needed)

## Projected Impact

Fixing items 1-8 would bring the score from **38/60 → ~57/60** (estimating some route status tests may have additional issues). Item 9 (4 subtests) deferred.

## Raw Test Output

Full conformance test output: [test-output-raw.log](test-output-raw.log)
