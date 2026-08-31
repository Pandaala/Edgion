# D0029 — Add Redis Token-Bucket Partial-Refill Integration Coverage

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0061, TokenBucket deny-balance correction
- Category: testing
- Canonical task: [`tasks/todo/add-redis-token-bucket-partial-refill-integration-coverage.md`](../../../todo/add-redis-token-bucket-partial-refill-integration-coverage.md)

## Evidence
Existing frozen Redis integration coverage does not exercise weighted TokenBucket partial refills across denied attempts. B0061 adds deterministic source-contract coverage, but a public real-Redis execution test is valuable for Lua state persistence and reply behavior.

## Candidate coverage
Add a new ignored, sleep-free real-Redis test using public `eval_policy`, explicit timestamps, and a unique key: drain the bucket, deny at one and two accumulated tokens for cost three, then allow at three; clean up the key.

## Why deferred
The Gateway quality campaign freezes existing integration files. This coverage is independent follow-up work and does not block the local production fix.
