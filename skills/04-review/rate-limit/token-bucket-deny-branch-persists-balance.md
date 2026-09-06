---
name: token-bucket-deny-branch-persists-balance
description: Use when reviewing Redis token-bucket findings about the deny branch writing 'tokens', redundant HMSET calls, or missing weighted partial-refill coverage; covers why both branches must persist the recomputed balance and where the two regression tests live.
status: fixed
finding_id: B0061 / D0029
closed: 2026-09-06
---

# Redis Token-Bucket Deny Branch Persists the Recomputed Balance — FIXED

## Conclusion

`TOKEN_BUCKET_SCRIPT` must persist **both** `tokens` and `ts` on the deny branch, exactly as
it does on the allow branch. Writing only `ts` discards every sub-cost refill, so a caller with
a weighted `cost > 1` can stay denied indefinitely no matter how much refill time elapses.

Fix suggestions of the form "the deny branch does not consume tokens, so the `HMSET` is
redundant — drop it and write only the timestamp" are **not accepted**.

## Core Rationale

**1. Advancing `ts` without storing `tokens` destroys the refill.**

`edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs` recomputes
`tokens = min(max_tokens, tokens + refill)` and sets `last_ts = now` whenever `refill > 0`.
That recomputation is shared by both branches. If the deny branch persists `ts` alone, the next
call reloads the *old* balance but measures elapsed time from the *new* timestamp, so each
denied attempt silently throws away the tokens it just earned. With `cost = 3` on a bucket
refilling one token per second, the balance oscillates between 0 and 1 forever and the caller
is never admitted.

**2. The failure mode is invisible in operation.**

A permanently denied weighted caller looks identical to a correctly enforced limit: the
plugin emits ordinary rate-limit denials and the metrics show the limiter working. Nothing
distinguishes "budget genuinely exhausted" from "budget can never be reached", which is why
this needs a regression test rather than operational alerting.

**3. Two regression layers, deliberately.**

- `scripts.rs` colocated test `token_bucket_persists_balance_and_timestamp_for_allow_and_deny`
  is a source-contract test: it anchors on the outer allow return/`else` and the deny
  return/`end` and requires the persistence call in each arm plus exactly two sites in the
  whole script. It runs with no network and catches an edit that deletes the write.
- `edgion-tests/tests/token_bucket_partial_refill_redis.rs` is the execution test: against the
  real Redis fixture it drains the bucket, denies at one and at two accumulated tokens for
  `cost = 3`, then requires an allow at three. It uses `eval_policy` with explicit `now_ms`
  values and a unique hash-tagged key, so it is deterministic and sleep-free. It is `#[ignore]`
  and runs only with the integration Redis fixture.

The source test alone is insufficient: a rewrite that restructures the Lua can satisfy or
carry along a string assertion while changing real behavior. The execution test alone is
insufficient: it does not run in ordinary `cargo test`.

## Fix Suggestions Not Accepted

- Drop the deny-branch `HMSET` as redundant — reintroduces the original bug.
- Persist `ts` only when the request is admitted — leaves refill time unaccounted after a deny.
- Replace the source-contract test with the real-Redis test — the latter is ignored by default.
- Prove the behavior with wall-clock sleeps instead of explicit `now_ms` — `eval_policy` takes
  the timestamp precisely so tests never race the clock.

## Re-evaluation Triggers

Re-open this decision only if:

- The token-bucket Lua stops storing balance and timestamp as separate hash fields.
- `eval_policy` loses its explicit `now_ms` parameter.

## Reference Cases

- B0061 (Gateway quality campaign) — production fix, commit `695f0ba9`
- D0029 / `tasks/todo/add-redis-token-bucket-partial-refill-integration-coverage.md` — the
  deferred real-Redis coverage, added 2026-09-06
- `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs`
- `edgion-tests/tests/token_bucket_partial_refill_redis.rs`
