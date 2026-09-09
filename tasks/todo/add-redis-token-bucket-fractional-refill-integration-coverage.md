# Add Redis Token-Bucket Fractional Refill Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Type | test |
| Priority | P2 |

## Problem

The shared Redis token-bucket script carries fractional refill credit in the
`refill_remainder` HASH field. Existing real-Redis coverage advances time by an exact whole-token
step, so it proves denied-balance persistence but cannot detect a regression that floors each
elapsed refill and discards the fractional numerator when advancing the timestamp.

## Requirements

- Add a new standalone real-Redis test file without modifying, deleting, renaming, or registering
  through any existing `edgion-tests/**` file.
- Drive `RedisRateLimiter::eval_policy` with explicit synthetic timestamps; do not use sleeps or
  wall-clock timing.
- Use a unique hash-tagged Redis key and clean up only that key.
- Prove a 3-token/1000-ms bucket sampled every 500 ms replenishes six tokens over two seconds,
  including state transitions through both denied and allowed branches.
- Seed or produce a legacy two-field `tokens`/`ts` HASH and verify a missing
  `refill_remainder` migrates as zero without resetting the stored balance.
- Verify reaching capacity clears fractional credit, a backward timestamp preserves the stored
  timestamp and remainder while still applying the current request, and retry milliseconds use
  the carried remainder with integer ceiling behavior.
- Inspect the stored `tokens`, `ts`, and `refill_remainder` fields where necessary, and retain the
  existing four-integer Lua reply and `PEXPIRE interval_ms * 2` contracts.
- Run against an isolated Redis fixture and document the exact opt-in command and environment
  requirements in the new test file.

## Preserved Boundaries

- Do not change production algorithms, Redis keys/hash slots, script-cache lifecycle,
  EVALSHA/NOSCRIPT fallback, LinkSys client ownership, failure policies, response headers,
  logging, metrics, or configuration schema.
- Do not fold the separate cross-caller numeric-domain work into this test task.
- Existing integration tests remain unchanged.

## Acceptance Criteria

- The new deterministic Redis test fails if fractional refill credit is dropped on either branch,
  retained past saturation, reset by clock rollback, or omitted from retry-time calculation.
- Legacy two-field state upgrades lazily and preserves its balance/timestamp.
- The new test cleans up its own data and passes repeatedly without depending on execution order.
