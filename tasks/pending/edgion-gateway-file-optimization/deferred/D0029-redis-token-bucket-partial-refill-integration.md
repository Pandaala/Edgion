# D0029 — Add Redis Token-Bucket Partial-Refill Integration Coverage

## Meta
- Status: done (2026-09-06)
- Severity: medium
- Discovered in: B0061, TokenBucket deny-balance correction
- Category: testing
- Closed by: `edgion-tests/tests/token_bucket_partial_refill_redis.rs`, registered in the integration runner; decision record [`skills/04-review/rate-limit/token-bucket-deny-branch-persists-balance.md`](../../../../skills/04-review/rate-limit/token-bucket-deny-branch-persists-balance.md). The canonical task file was deleted on close.

## Evidence
Existing frozen Redis integration coverage does not exercise weighted TokenBucket partial refills across denied attempts. B0061 adds deterministic source-contract coverage, but a public real-Redis execution test is valuable for Lua state persistence and reply behavior.

## Candidate coverage
Add a new ignored, sleep-free real-Redis test using public `eval_policy`, explicit timestamps, and a unique key: drain the bucket, deny at one and two accumulated tokens for cost three, then allow at three; clean up the key.

## Why deferred
The Gateway quality campaign freezes existing integration files. This coverage is independent follow-up work and does not block the local production fix.
