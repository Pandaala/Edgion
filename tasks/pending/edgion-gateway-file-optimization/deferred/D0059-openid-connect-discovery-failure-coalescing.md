# D0059 — Coalesce and Back Off Failed OpenID Connect Discovery Fetches

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0092, OpenID Connect final review
- Category: reliability-concurrency
- Canonical task: [`tasks/todo/openid-connect-discovery-failure-coalescing.md`](../../../todo/openid-connect-discovery-failure-coalescing.md)

## Evidence
The discovery refresh mutex serializes cold callers, but a failed discovery fetch publishes no shared outcome. After the first failure releases the mutex, each waiting caller observes an empty cache and performs another fetch, so a burst can produce one provider timeout per request. The caller that owns the fetch future also owns cancellation; dropping it cancels the attempt instead of allowing other waiters to observe a shared result.

## Why deferred
The correct fix needs an explicit failure-state and cancellation-ownership design spanning lazy fetch, prewarm, retry timing, recovery publication, and redacted error delivery. Adding only an `Arc` or holding the current mutex longer would not make failures shareable and could poison recovery or retain a cancelled leader. This is independent from B0092's successful-snapshot ownership optimization.

## Integration-test impact
No new integration issue is required. A provider-local HTTP fixture plus deterministic Tokio tests can prove one outbound fetch for concurrent failures, waiter cancellation behavior, bounded backoff, retry, and successful recovery without changing existing frozen integration tests.
