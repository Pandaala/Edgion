---
name: batch-conformance-debugging
description: Log collection, analysis, and diagnosis methods for batch conformance test failures, used to localize cross-test state leakage and concurrency issues
---

# Batch Conformance Test Debugging

Use this guide when tests pass individually but fail when run in batch. The core idea: **collect three streams of logs → identify key patterns → correlate timelines → locate root cause**.

## 1. Log collection

Before running a batch, start three log-collection streams; all logs carry timestamps:

```bash
# Terminal 1: Controller logs
kubectl logs -f deployment/edgion-controller -n edgion-system --timestamps 2>&1 | tee /tmp/controller-batch.log

# Terminal 2: Gateway logs
kubectl logs -f deployment/edgion-gateway -n edgion-system --timestamps 2>&1 | tee /tmp/gateway-batch.log

# Terminal 3: Test output
./scripts/orbstack-dev.sh test --core 2>&1 | tee /tmp/test-batch.log
```

For comparison, run the failing test alone:

```bash
kubectl logs -f deployment/edgion-controller -n edgion-system --timestamps 2>&1 | tee /tmp/controller-single.log &
kubectl logs -f deployment/edgion-gateway -n edgion-system --timestamps 2>&1 | tee /tmp/gateway-single.log &
./scripts/orbstack-dev.sh test --run-test <FailingTestName> 2>&1 | tee /tmp/test-single.log
```

## 2. Key log patterns

### Controller side

| Pattern | Meaning | Search method |
|---------|---------|---------------|
| Queue-depth warning | Requeue queue backlog (warn at 80% capacity) | `grep -i "queue.*depth\|queue.*warn\|capacity"` |
| Requeue storm | The same key is repeatedly requeued | `grep "enqueue\|requeue" \| sort \| uniq -c \| sort -rn` |
| Status-patch conflict | Status writes fail due to ResourceVersion conflict | `grep -i "conflict\|resource.version\|status.*fail\|status.*error"` |
| Delete/create timing overlap | A resource is recreated before its deletion completes | `grep "on_delete\|on_change\|on_add"`; look at the time interval between events for the same key |
| Trigger-chain depth | Cascaded requeue truncated due to depth | `grep -i "trigger.*cycle\|trigger.*depth\|cascade"` |
| Processing duration | A single work item takes too long | `grep -i "process.*ms\|elapsed\|slow"` |

### Gateway side

| Pattern | Meaning | Search method |
|---------|---------|---------------|
| `TOO_OLD_VERSION` | Event buffer overflow, triggering full re-list | `grep "TOO_OLD_VERSION\|EVENTS_LOST"` |
| `SERVER_RELOAD` | Controller restart causes full re-sync | `grep "SERVER_RELOAD\|SERVER_ID_MISMATCH\|server_id"` |
| Re-list trigger | Gateway issues a full list request (not at startup) | `grep -i "re-list\|full.*list\|list.*request"` |
| Sync version jump | Version numbers are not consecutively increasing | `grep "sync_version\|from_version"` |
| Routing-table updates | When routes are added / removed | `grep -i "route.*add\|route.*delete\|route.*update\|partial_update"` |
| Backoff trigger | Reconnection wait after a disconnect | `grep -i "backoff\|reconnect\|retry"` |

### Test output

| Pattern | Meaning | Search method |
|---------|---------|---------------|
| Unexpected 200 | Old route still active; request was incorrectly forwarded | `grep "expected 404.*got 200\|unexpected.*200"` |
| Unexpected 404 | New route hasn't taken effect yet | `grep "expected 200.*got 404\|unexpected.*404"` |
| Status condition mismatch | Status is from the previous test | `grep "condition.*expected\|status.*mismatch\|Accepted.*False"` |
| Timeout | Waiting for a condition timed out | `grep -i "timeout\|timed out\|deadline"` |

## 3. Timeline-correlation analysis

**Goal:** align the test failure time with controller / gateway logs to determine which failure mode applies.

### Steps

1. **Find the failure timestamps in the test output**
   ```bash
   grep -E "FAIL|--- FAIL" /tmp/test-batch.log
   ```

2. **Look in the controller log for events in that time window (±30 seconds)**
   ```bash
   # Assuming the failure was at 14:32:15
   awk '/14:31:45/,/14:32:45/' /tmp/controller-batch.log
   ```

3. **Look in the gateway log for sync events in the same time window**
   ```bash
   awk '/14:31:45/,/14:32:45/' /tmp/gateway-batch.log
   ```

4. **Identify the failure mode**

| Observation | Failure mode | Root-cause direction |
|-------------|--------------|----------------------|
| The Route delete event for test N appears only after test N+1 has started | Stale route lingering | Controller processing delay or gateway sync delay |
| The Route create event for test N+1 has been processed but the gateway didn't receive it | New route did not arrive | gRPC sync delay or event lost |
| Status patch is written only after test N+1's assertion | Status lag | Status-write queue backlog or conflict retries |
| The same key is repeatedly enqueued/dequeued | Requeue storm | Cascaded requeue amplification |

## 4. Single vs. batch comparison

For the same failing test:

```bash
# Compare the controller log timing for that test's resources
diff <(grep "same-namespace\|httproute" /tmp/controller-single.log) \
     <(grep "same-namespace\|httproute" /tmp/controller-batch.log)
```

Focus on:
- **Event-interval differences**: in the batch run, is the gap between delete and recreate shorter?
- **Queue-depth differences**: does the batch run hit a backlog the single run doesn't?
- **Extra events**: in the batch run, are there interleaved resource events from other tests?

## 5. Diagnostic-conclusion template

After analysis, record using this format:

```
Test: <TestName>
Failure mode: stale route lingering / new route did not arrive / status lag / requeue storm
Evidence:
  - [timestamp] controller: <key log line>
  - [timestamp] gateway: <key log line>
  - [timestamp] test: <failed assertion>
Timing issue: the gap between <event> in test N and <event> in test N+1 is only <X>ms
Suggested fix direction: <specific suggestion>
```

## Related files

- Batch fix plan: `tasks/todo/conformance-fix-batches/README.md`
- Batch 7 details: `tasks/todo/conformance-fix-batches/batch-07-batch-concurrency.md`
- Controller workqueue: `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs`
- Gateway sync: `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`
- Conformance test entry point: `edgion-tests/conformance/conformance_test.go`
