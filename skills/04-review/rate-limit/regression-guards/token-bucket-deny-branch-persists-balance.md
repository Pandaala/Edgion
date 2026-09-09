---
name: token-bucket-deny-branch-persists-balance
description: Use when reviewing Redis token-bucket findings about complete state persistence, fractional refill carry, or weighted partial-refill coverage.
status: fixed
finding_id: B0061 / D0029
closed: 2026-09-06
knowledge_type: regression-guards
---

# Redis Token-Bucket Deny Branch Persists the Recomputed Balance — FIXED

## Conclusion

`TOKEN_BUCKET_SCRIPT` must persist the complete refill state — `tokens`, `ts`, and
`refill_remainder` — on both the allow and deny branches. Writing only `ts` discards every
sub-cost refill. Resetting `ts` after an integer refill without carrying the fractional
numerator discards elapsed-time credit on both branches and can materially reduce throughput.

Fix suggestions of the form "the deny branch does not consume tokens, so the `HMSET` is
redundant — drop it and write only the timestamp" are **not accepted**.

## Core Rationale

**1. Advancing `ts` without storing `tokens` destroys the refill.**

`edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs` recomputes the balance before
the allow/deny decision. If the deny branch persists `ts` alone, the next call reloads the
*old* balance but measures elapsed time from the *new* timestamp, so each denied attempt
silently throws away the whole tokens it just earned. With `cost = 3` on a bucket refilling
one token per second, the balance can oscillate between 0 and 1 forever.

The fractional remainder is a separate invariant. For a 3-token/1000-ms bucket sampled every
500 ms, each step earns 1.5 tokens. Flooring to one token and resetting `ts` loses half a token
per step, so only four tokens refill over two seconds instead of six. The script therefore
carries `elapsed * refill_rate % interval_ms` into the next call. It clears that remainder when
the pre-consumption balance reaches capacity, because a full bucket cannot bank additional
credit. A backward timestamp consumes or denies against the stored balance without moving
`ts` backward or changing the remainder.

Existing two-field hashes migrate lazily: a missing `refill_remainder` means zero. This changes
neither Redis keys nor the one-key atomic execution boundary.

**2. The failure mode is invisible in operation.**

A permanently denied weighted caller looks identical to a correctly enforced limit: the
plugin emits ordinary rate-limit denials and the metrics show the limiter working. Nothing
distinguishes "budget genuinely exhausted" from "budget can never be reached", which is why
this needs a regression test rather than operational alerting.

**3. Regression layers are deliberately distinct.**

- `scripts.rs` colocated tests require complete state persistence in each branch, anchor the
  carry/retry formulas in the Lua source, and exercise a matching integer model across
  fractional cadence, migration, saturation, clock rollback, and retry rounding. They run
  without a network.
- `edgion-tests/tests/token_bucket_partial_refill_redis.rs` is the execution test: against the
  real Redis fixture it drains the bucket, denies at one and at two accumulated tokens for
  `cost = 3`, then requires an allow at three. It uses `eval_policy` with explicit `now_ms`
  values and a unique hash-tagged key, so it is deterministic and sleep-free. It is `#[ignore]`
  and runs only with the integration Redis fixture.
- `edgion-tests/tests/token_bucket_fractional_refill_redis.rs` exercises the fractional state
  against the same real Redis fixture. It proves a 3-token/1000-ms bucket sampled every 500 ms
  replenishes six tokens over two seconds across deny and allow branches, and covers lazy
  two-field migration, saturation credit clearing, clock rollback, and retry rounding. Each
  scenario uses a unique hash-tagged key and explicit timestamps.

The two execution tests cover distinct boundaries: the partial-refill test proves whole-token
balance persistence for weighted requests, while the fractional-refill test detects lost elapsed
credit and validates the complete three-field state lifecycle.

The source test alone is insufficient: a rewrite that restructures the Lua can satisfy or
carry along a string assertion while changing real behavior. The execution test alone is
insufficient: it does not run in ordinary `cargo test`.

## Fix Suggestions Not Accepted

- Drop the deny-branch `HMSET` as redundant — reintroduces the original bug.
- Persist `ts` only when the request is admitted — leaves refill time unaccounted after a deny.
- Reset `ts` after flooring a refill without persisting the fractional numerator — reduces the
  configured refill rate under common request cadences.
- Retain refill remainder while saturated — banks credit beyond bucket capacity.
- Replace the source-contract test with the real-Redis test — the latter is ignored by default.
- Prove the behavior with wall-clock sleeps instead of explicit `now_ms` — `eval_policy` takes
  the timestamp precisely so tests never race the clock.

The separate Redis rate-limiter numeric-domain task owns validation of all Lua/Rust arithmetic
bounds across HTTP and Stream callers. This guard relies on that intended bounded domain and does
not replace or narrow that task.

## Re-evaluation Triggers

Re-open this decision only if:

- The token-bucket Lua stops storing balance, timestamp, and fractional remainder as separate
  hash fields.
- `eval_policy` loses its explicit `now_ms` parameter.

## Reference Cases

- B0061 (Gateway quality campaign) — production fix, commit `695f0ba9`
- D0029 / `tasks/todo/add-redis-token-bucket-partial-refill-integration-coverage.md` — the
  deferred real-Redis coverage, added 2026-09-06
- `tasks/todo/define-redis-rate-limiter-numeric-domain/define-redis-rate-limiter-numeric-domain.md`
  — canonical cross-caller numeric bounds
- `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs`
- `edgion-tests/tests/token_bucket_partial_refill_redis.rs`
- `edgion-tests/tests/token_bucket_fractional_refill_redis.rs`

## Resolution references

- `D0029` — 2026-09-06.
- Fractional real-Redis execution coverage — 2026-09-09.
