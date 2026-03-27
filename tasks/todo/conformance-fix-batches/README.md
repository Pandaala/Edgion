# Conformance Fix Batches

## Current Status

| Metric | Value |
|--------|-------|
| Individual runs | 43/60 passed, 17 failed |
| Batch run | 21/60 (22 tests pass individually but fail in batch) |
| Target | 57-58/60 |

## Batch Order

| Batch | Title | Tests Fixed | Component | Effort |
|-------|-------|-------------|-----------|--------|
| 1 | [GRPCRoute error response](batch-01-grpcroute.md) | 4 (23 subtests) | Gateway | Small |
| 2 | [Route status persistence](batch-02-route-status.md) | 6 | Controller | Medium |
| 3 | [HTTPS catch-all + port 8080](batch-03-https-port.md) | 2 (8 subtests) | Gateway | Small |
| 4 | [Timeout "0s" semantics](batch-04-timeout.md) | 2 (3 subtests) | Gateway | Medium |
| 5 | [PercentageMirror field](batch-05-percentage-mirror.md) | 1 (2 subtests) | Both | Small |
| 6 | [HostnameIntersection attachedRoutes](batch-06-hostname-intersection.md) | 1 (1 subtest) | Controller | Small |
| 7 | [Batch concurrency](batch-07-batch-concurrency.md) | 22 (pass individually, fail in batch) | Controller | Investigation |

**Total individually failing:** 16 tests across batches 1-6 (+ 1 deferred)
**Total batch-only failures:** 22 tests in batch 7

## Deferred

- **HTTPRouteListenerPortMatching** (4 subtests) -- Requires dynamic listener creation at runtime. This is an architectural limitation: the gateway binds listeners at startup based on Gateway resources. Supporting arbitrary port listeners on-the-fly is a P4 change. Do not fix now.
- **HTTPRouteRedirectPortAndScheme** port 8080 subtests (3 subtests) -- Same dynamic listener limitation. The `CustomDialContext` only maps pre-configured ports.

## Workflow

1. Work through batches in order (1 through 6).
2. After each batch, run the individual test(s) to confirm they pass.
3. After all 6 batches, run the core batch to measure batch score improvement.
4. Tackle batch 7 (concurrency investigation) last -- the earlier fixes (especially batch 2) may partially resolve batch-only failures.

### Commands Reference

```bash
# Rebuild components
./scripts/orbstack-dev.sh rebuild c    # controller only
./scripts/orbstack-dev.sh rebuild g    # gateway only
./scripts/orbstack-dev.sh rebuild c g  # both

# Run individual test
./scripts/orbstack-dev.sh test --run-test TestName

# Run full core batch
./scripts/orbstack-dev.sh test --core

# Run all conformance tests
./scripts/orbstack-dev.sh test
```

## Analysis Source

Detailed root-cause analysis for each fix is in `tasks/todo/conformance-fixes/` (files 01-08).
