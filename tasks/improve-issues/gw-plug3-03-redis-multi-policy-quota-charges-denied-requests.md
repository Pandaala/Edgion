# gw-plug3-03: Redis multi-policy quotas charge long windows for requests that a short window rejects (one script call per policy, gateway clock)

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs:748-797` (`RedisRateLimiter::check`: per-policy loop, no early stop, `SystemTime::now()`)
  - `edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs:701-746` (`eval_policy`: one EVALSHA per policy)
  - `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs:340-384` (sliding window commits on allow), `:400-418` (fixed window `INCRBY`), `:430-487` (token bucket; `now`/`last_ts` from the caller)
  - Consumer: `edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs:171` (and the stream / GraphQL consumers of the same core)
  - Doc promise: `docs/en/user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md:155,292`

## Summary
A RateLimitRedis quota may list up to 10 policies. The doc example combines "50 req/s **and** 5000
req/h for the same API key". `check()` runs one independent Lua script per policy, sequentially, and
always runs all of them. Each script commits its own counter when that policy alone allows the
request. When the short policy denies, every longer policy still records a successful debit. A client
that bursts above the per-second limit therefore drains the hourly quota with rejected requests and is
locked out for the rest of the hour after a few seconds. The algorithms also take `now` from each
gateway's wall clock rather than from Redis, so replicas with clock skew disagree about window and
refill boundaries.

## Current behavior (evidence)
- `check()` loops `for (idx, policy) in policies.iter().enumerate()` and calls `eval_policy` for each
  policy (`mod.rs:768-794`). It merges results with `RateLimitResult::merge`, and nothing stops the
  loop after a denial.
- Every script commits only on its own local allow: sliding window `cur = cur + cost` + `HMSET`
  (`scripts.rs:377`), fixed window `INCRBY` (`:411`), token bucket `tokens = tokens - cost`
  (`:471`). Policy 0 (50/s) denying does not stop policy 1 (5000/h) from incrementing.
- Worked example from the user doc (`rate-limit-redis.md:292`): a client sends 1000 req/s.
  - Second policy: 50 allowed and 950 denied per second.
  - Hour policy: about 1000 debited per second, because it allows each request until exhausted.
  - After about 5 s the hour bucket reaches 5000. The client is then denied for up to an hour, after
    receiving only about 250 successful responses. The doc says "Both policies are evaluated
    independently — either one triggering causes rejection", which suggests the
    `min(50/s, 5000/h)` envelope, not this lockout.
- Latency: N policies cost N sequential Redis round trips per request (`eval_policy` awaits each one),
  even though all keys of one quota could share one hash-tagged slot.
- Clock: `now_ms = SystemTime::now()` on the gateway (`mod.rs:757`) is passed as `ARGV`.
  - Token bucket only refills when `now >= last_ts` (`scripts.rs:451`). One replica that is ahead
    by Δ stores a future `last_ts`, and every other replica gets no refill for Δ.
  - Sliding window uses `now % window` to rotate. Skewed replicas disagree on window boundaries and
    weights, so counts are slightly wrong near boundaries.

## Industry comparison
- **Kong rate-limiting** (Redis policy) reads usage for all configured periods first. It denies when
  any period is exhausted and increments every period only when the request is admitted. Denied
  requests never consume the longer windows.
- **Envoy global rate limit (RLS)** sends all descriptors in one RPC. The service decides atomically
  per request (one pipeline), and Envoy local rate limit uses a shared clock source.
- Redis-based limiters such as redis-cell, and the common token-bucket recipes, read `redis.call('TIME')`
  inside the script so every client shares one clock. Script effects replication has made `TIME` safe
  since Redis 5.

## Impact
- Multi-window quotas (per-second burst plus per-hour or per-day budget), the main reason to have
  `policies` as a list, punish bursty but legitimate clients far beyond the configured envelope. An
  attacker who knows a victim's key, or uses a shared default key, can exhaust the long-window budget
  for everyone in seconds.
- Per-request latency grows linearly with the number of policies.
- Clock skew between gateway pods silently lowers the effective token-bucket rate.

## Suggested direction
1. Evaluate all policies of one quota in **one** Lua script over hash-tagged keys
   (`{prefix:quota:key}`). Run in two phases inside the script: compute every policy's decision
   without writing, then commit all counters only when every policy allows. This gives the documented
   AND semantics atomically and in one RTT.
2. Use `redis.call('TIME')` inside the scripts instead of a caller-supplied `now`. Keep the caller
   value only for tests via an explicit flag.
3. Keep the documented cross-quota non-transactional behavior (see Notes) as is, but state precisely
   in the user doc that it applies across quotas, not across policies within one quota.

## Notes
- `skills/04-review/rate-limit/regression-guards/redis-named-quotas.md` records "Individual Lua
  counters are atomic; multiple counters are not transactional. Later rejection/failure retains
  earlier debits", and "Each quota retains existing all-policy core evaluation". This finding gives
  new evidence that the accepted tradeoff is wrong **within a quota**:
  - The cited case there is cross-quota rejection or timeout after an earlier successful debit.
  - The case here is a deterministic, unbounded drain of a long window by requests that a sibling
    policy in the same quota rejects. It happens on every request, not only on errors.
  - The documented user example (`50/s AND 5000/h`) is broken by it.
  - Making one quota's policies atomic needs no cross-key or cross-quota transaction, because all of
    its keys can share one hash tag.
- `token-bucket-deny-branch-persists-balance.md` and `redis-script-cache-lifecycle.md` cover other
  script aspects. `gw-plugin-04` covers the local (in-memory) limiter only.
- `tasks/todo/ai-todo/quota-modes-and-inflight-reservations.md` concerns AI token quotas
  (cumulative / credential scripts), not these request-rate scripts.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `redis_rate_limiter/mod.rs:778-797`: `check()` awaits `eval_policy` for every policy and merges results, with no early stop on denial; one EVALSHA per policy (`mod.rs:710-755`).
- Lua scripts commit on local allow regardless of sibling policies: sliding window `HMSET` (`scripts.rs:377`), fixed window `INCRBY` (`scripts.rs:411`), token bucket `tokens - cost` (`scripts.rs:471`). So a short-window denial still debits the long window.
- The user doc promises AND semantics for the burst plus hourly example (`rate-limit-redis.md:155,292`). `redis-named-quotas.md` accepts non-transactional debits *across quotas* ("stop at first denial") and says each quota keeps "all-policy core evaluation". It does not address deterministic draining of long windows by requests that a sibling policy in the same quota denies, so this is new evidence.
- `now_ms` comes from the gateway `SystemTime::now()` (`mod.rs:766`). The clock-skew point is valid but secondary.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `redis_rate_limiter/mod.rs` moved from fred to redis-rs (`eval_script` now builds `EVALSHA`/`EVAL` via `exec_until`), but `check()` (`:748-797`) still awaits `eval_policy` (`:701-746`) for every policy in the loop at `:768-794` and merges, with no early stop.
- `scripts.rs` has no diff; each script still commits on local allow. `now_ms` still from gateway `SystemTime::now()` (`:757`).
- Location/evidence line references updated to the current file.
