# Add Redis Token-Bucket Partial-Refill Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | test |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add a new ignored real-Redis integration test that proves weighted TokenBucket requests accumulate partial refills across denied attempts. The test validates B0061's Lua persistence fix through the public shared evaluator with explicit timestamps, without sleeps or changes to existing integration scenarios.

## Requirements

- Add a new standalone ignored real-Redis test file; do not weaken or rewrite existing integration tests.
- Reuse the existing Redis fixture and canonical RedisLinkClient setup.
- Use `eval_policy` with explicit `now_ms` values and a unique same-slot test key; never depend on wall-clock sleeps.
- Drain the bucket, then prove one-token and two-token partial refills both deny a weighted cost of three while preserving balance, and the third accumulated token allows the request.
- Assert the public reply fields relevant to allow/deny, remaining balance, and retry behavior without coupling to private Lua source text.
- Delete the unique Redis key during cleanup even after a failed assertion when practical.
- Keep the test ignored unless the existing real-Redis harness explicitly enables it.

## Scope

### In scope

- One new real-Redis integration test file and minimal future runner registration only if automatic discovery is unavailable.
- Public `eval_policy` TokenBucket behavior with explicit timestamps.

### Out of scope

- Changing production Lua, algorithms, keys, failure policy, Redis topology, or the numeric-domain contract.
- Editing existing integration assertions or using fixed sleeps.

## Acceptance criteria

- The explicit sequence demonstrates drained balance, deny at one token, deny at two accumulated tokens, and allow at three accumulated tokens for cost three.
- The test is deterministic, sleep-free, uses a unique key, and cleans up Redis state.
- Existing integration files and scenarios remain unchanged except minimal registration if the harness cannot discover the new file automatically.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Record future coverage instead of editing integration tests in B0061. | Current real-Redis coverage exercises other limiter paths and cannot prove partial-refill persistence across deny. |
