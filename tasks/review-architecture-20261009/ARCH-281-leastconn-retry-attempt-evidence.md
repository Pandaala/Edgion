---
id: ARCH-281
status: candidate
kind: behavior-defect
priority: P2
partition: PART-555
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---
# Verify the LeastConn retry storm actually contains the configured retry attempts

`LBLeastConnRetryOnStatusTestSuite::test_retry_storm_then_healthy_distribution` claims that an eight-request 503 storm exercises retry abandonment and connection-ledger cleanup before subsequent healthy traffic. Its first-phase verdict observes only final HTTP statuses. Final 503 does not establish whether any retry happened, leaving the integration wiring invariant without an observer.

## Concrete evidence and distinction

The actual fixture configures `retry.attempts: 3`, `codes: [503]` and LeastConn for two reachable endpoints. Each storm request calls `/status/503` with a readable W3C marker. The suite joins all requests and fails transport/join errors or any final status other than 503. It neither requests stored access logs nor checks an upstream attempt chain. A regression that ignores the retry policy and forwards the first backend 503 yields eight expected final 503 responses; healthy requests can then distribute normally and satisfy the remainder. Even after correcting the separate lifetime-metric issue in ARCH280, this missing-attempt witness remains.

The backend fixture returns 503 on every attempt. That observable response is the same after one attempt or four. The Gateway's existing terminal LB test data records final endpoint/hash information, not an attempt count. Reading those existing distribution counters cannot prove the storm retried.

This is not a claim that current Gateway retries are broken or that all ledger mutation lacks tests. Actual `lb::attempt` and its unit tests separately prove scoring-policy increment, idempotent commit/finalize, cancellation-safe release and no scoring by the guard. Those direct unit tests cannot establish that this integration case entered the configured retry branch. The current EWMA retry suite provides a concrete existing harness pattern: one unique request marker, `access_log: test_store`, final 503, exactly four stored upstream attempts with policy/status and `RetryOnStatus` on the first three only.

## Bounded improvement

Keep the eight-request concurrent storm and its status/error checks, and add bounded inspection through the existing `AccessLogClient` for every storm request. Give each invocation a unique canonical W3C marker so old stored evidence cannot substitute. Opt those requests into the existing integration-only access-log store. Require exactly four upstream attempts, each with 503 and LeastConn attribution, the first three classified `RetryOnStatus`, and the final attempt without that abandoned-response marker. Missing, malformed, truncated, wrong-policy or timed-out evidence must fail before claiming a verified storm. Preserve any existing accounting/unit contract rather than treating four attempts alone as proof of internal counter correctness.

Reuse current access-log retrieval, normalized identity and existing test-store cleanup where applicable; no new gateway metrics, traces, logging, response aggregation or generic observer is needed. Preserve suite/case identities, retry fixture, endpoint topology, concurrency, healthy-phase checks and framework failure reporting. Coordinate with ARCH280 for the independent healthy-phase delta; neither remedy substitutes for the other.

Future verification must reject a valid final 503 accompanied by one attempt, wrong retry markers/count/policy, stale request identity and absent store evidence; the exact configured four-attempt chain must pass. Existing original per-attempt retry guidance, actual access-log marker/retrieval consumers, current EWMA integration example, fixture, production LB guard and unit bodies were read. No runtime reproduction, builds/tests, source edits, CI failure or performance claim is made.
