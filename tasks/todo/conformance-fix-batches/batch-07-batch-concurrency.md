# Batch 7: Batch Concurrency Issue

## Status: [ ] Not Started

## Tests to Fix
- 22 tests that pass individually but fail when run together in the conformance batch (score drops from 43/60 to 21/60)

## Problem

When the full conformance suite runs, 22 tests that pass individually start failing. This points to controller timing/requeue issues under load. The conformance suite likely:

1. **Runs tests sequentially but rapidly** -- each test creates Gateway + Route resources, runs assertions, then deletes them. The next test starts immediately.
2. **Reuses resource names** -- multiple tests may use the same Gateway name (`same-namespace`) or similar resource names, causing stale config from a previous test to leak into the next.
3. **Overwhelms the controller's requeue pipeline** -- rapid create/delete/create cycles may cause the controller to fall behind, serving stale route tables or status conditions.

Specific failure patterns to investigate:
- **Stale routes:** A route from test N is still active when test N+1 starts, causing unexpected 200s or wrong backends.
- **Missing routes:** A route from test N+1 hasn't been synced yet, causing unexpected 404s.
- **Stale status:** Status conditions from test N's resources haven't been cleared, causing test N+1's status assertions to see wrong values.

## Investigation Steps

### Step 1: Identify which 22 tests fail in batch

```bash
# Run full batch and capture results
./scripts/orbstack-dev.sh test --core 2>&1 | tee /tmp/batch-results.log

# Compare with individual results to find the 22 that regress
```

### Step 2: Check controller logs during batch run

```bash
# Watch controller logs during batch run
kubectl logs -f deployment/edgion-controller -n edgion-system 2>&1 | tee /tmp/controller-batch.log

# Look for:
# - Requeue backlog (queue depth growing)
# - Slow status writes (latency > 5s)
# - "Unknown kind" errors
# - Resource version conflicts on status patches
```

### Step 3: Check for resource cleanup timing

The conformance suite deletes resources after each test. If the controller hasn't fully processed the deletion before the next test creates resources with the same name:
- The gateway may still have old routes cached.
- The controller may see a "create" as an "update" to a resource it's still cleaning up.

**Investigation:** Add timestamps to controller logs around `on_change`/`on_delete` events. Check if there's overlap between test N's cleanup and test N+1's setup.

### Step 4: Check gateway config sync latency

The gateway receives config updates via gRPC from the controller. If the sync has latency:
- Routes from the previous test may still be active.
- Routes from the current test may not be active yet.

```bash
# Check gRPC sync latency
kubectl logs -f deployment/edgion-gateway -n edgion-system 2>&1 | grep -i "sync\|config\|update"
```

### Step 5: Potential fixes (apply after investigation confirms root cause)

1. **Increase requeue worker count** -- if the controller has a fixed-size worker pool for requeue events, increase it.
2. **Add resource version tracking** -- ensure the gateway discards stale config updates that arrive out of order.
3. **Reduce status write debounce** -- if status writes are batched, reduce the batch interval.
4. **Add finalizer-based cleanup** -- ensure resources are fully cleaned up before deletion completes.
5. **Add readiness gates** -- the conformance suite may support waiting for resource readiness before sending traffic.

### Step 6: Re-evaluate after batches 1-6

Batches 1-6 fix 16 individually-failing tests. Some of those fixes (especially batch 2's status persistence fix) may also resolve batch-only failures. Re-run the full batch after completing batches 1-6 before investing heavily in this investigation.

## Build & Deploy
```bash
# Depends on findings -- likely both controller and gateway
./scripts/orbstack-dev.sh rebuild c g
```

## Verify
```bash
# Run full core batch
./scripts/orbstack-dev.sh test --core

# Compare score with pre-fix baseline
```

## Result
- [ ] Individual tests pass
- [ ] Core batch score improved
- Previous score: 21/60
- New score: __/60
