# gw-plugin-04: RateLimitLocal is a fixed window advertised as sliding; zero or sub-second intervals disable limiting or can panic

- **Category**: design-error
- **Severity**: P2 (downgraded from P1 in independent verification)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/rate_limit_local/plugin.rs:109-115,117-127,248-283`; `edgion-resources/src/resources/edgion_plugins/plugin_configs/rate_limit_local.rs:545-548,576`; `edgion-resources/src/duration.rs:64-121`; dependency `pingora-limits/src/rate.rs` (`observe`, `maybe_reset`)

## Summary
`RateLimitLocal` decides with `pingora_limits::rate::Rate::observe`, which returns the count of the
**current fixed interval only**. The red/blue second slot is used by `Rate::rate()` and
`rate_with()`, which this plugin never calls. The behavior is therefore a fixed window with the
classic 2× boundary burst, while the user doc promises a "sliding window". On top of that,
`interval` has no lower bound. `0ms` is accepted, and `pingora_limits` then rotates the window on
every call, which effectively disables limiting. Any interval under 1 s exposes an upstream
`assert!(new >= now - 1000)` that underflows during the first second of each `Rate` lifetime and
panics the request task under contention.

## Current behavior (evidence)
- `plugin.rs:248`: `let curr_count = rate.observe(&limit_key, 1);` then `exceeded = curr_count > rate_limit`.
- `pingora-limits rate.rs`: `observe` calls `maybe_reset()` and then
  `self.current(..).incr(key, events)`. The comment reads "return number of events seen so far in
  the current interval". The previous slot is only read by `rate()`/`rate_with()`.
- The doc (`docs/en/user-guide/edgion-plugins/rate-limit-local.md:17,468-483`) says "Sliding window
  - Dual-slot (red/blue) design for smooth sliding window rate limiting". That is false for this
  call path. A client can send `rate` requests at the end of window *k* and `rate` more at the start
  of *k+1*.
- Interval validation (`rate_limit_local.rs:545-548,576`) only checks that `parse_duration` succeeds.
  `parse_duration("0ms")` returns `Duration::ZERO` (`duration.rs:96-121`), and no minimum is
  enforced. With `reset_interval_ms == 0`, `maybe_reset` resets on every call because
  `past_ms < 0` is never true. Each `observe` flips slots and clears one, so `curr_count` stays
  around 1 and nothing is ever rejected. A typo silently turns the limiter off.
- With `0 < interval < 1s`, resets happen while `now < 1000 ms` after `Rate` creation. A losing
  `compare_exchange` reaches `assert!(new >= now - 1000)`. Here `now - 1000` underflows `u64`: it
  wraps in release so the assert fails, and with overflow checks it panics on the subtraction. The
  `Rate` is created lazily (`get_rate`, `OnceLock`) for every new plugin generation, so every
  config reload reopens this window. The plugin runtime has no `catch_unwind`.
- `X-RateLimit-Reset` and `Retry-After` are computed as `now + interval` (`:121-127`, `:282`), not
  the end of the current window. They are therefore wrong for every request except one that lands
  exactly at a window boundary.

## Industry comparison
- **Envoy `local_ratelimit`**: a token bucket (`max_tokens`, `tokens_per_fill`, `fill_interval`,
  with a 50 ms minimum enforced). No boundary burst, and the reset hint is exact.
- **Kong `rate-limiting`**: fixed windows, but documented as such and aligned to wall-clock
  second, minute, or hour, so `RateLimit-Reset` is exact. **Kong Advanced** and **APISIX
  `limit-count`/`limit-req`** offer sliding-window (weighted previous + current) or leaky-bucket
  variants.
- The weighted sliding estimate is available directly from the same dependency:
  `Rate::rate_with` exposes `prev_samples`, `curr_samples`, and `current_interval_fraction`.

## Impact
Operators sizing a protective limit get up to 2× the configured rate in bursts, contrary to the
documentation. A `0ms`, `0s`, or `0m` interval passes Controller validation (`Accepted=True`) and
disables the limiter. Sub-second intervals, which are plausible for "per 100 ms" smoothing, can drop
requests by panicking right after every deploy or config update.

## Suggested direction
1. Reject `interval == 0`. Enforce a floor of ≥ 1 s. If sub-second smoothing is wanted, implement
   it as a token bucket instead, so the upstream `now - 1000` hazard is never reachable. Put the
   check in the shared Resources validation so Controller and Gateway agree.
2. Either compute a real sliding estimate:
   `prev * (1 - fraction) + curr`, via `observe` + `rate_with`, as Cloudflare and Kong Advanced do.
   Or correct the docs to say "fixed window" and align reset hints to the actual window end, which
   requires tracking the window start or using `rate_with`'s fraction.
3. Optionally report the upstream underflow to `pingora-limits`, using a `saturating_sub`.

## Notes
- Checked `skills/04-review/rate-limit/` (SKILL index, `local-allowance-redis-overflow`,
  `token-bucket-deny-branch-persists-balance`, …). The Redis token bucket is reviewed. Neither the
  local fixed-window semantics nor interval bounds are covered.
- CMS over-estimation under high key cardinality (false 429s for innocent keys once distinct keys
  exceed `estimatorSlotsK`) is documented as "always conservative" and was not filed separately.
  Note, though, that for a limiter "conservative" means denying legitimate clients.

## Verification (independent)

- **Verdict**: DOWNGRADED (P1 → P2)
- `edgion-gateway/src/plugins/http/rate_limit_local/plugin.rs:248-250`: the decision uses only `rate.observe(..)` (current-slot count), never `rate()`/`rate_with()`, so it is a fixed window; the user doc (`docs/en/user-guide/edgion-plugins/rate-limit-local.md:17,468-470`) advertises a sliding window.
- `edgion-resources/.../plugin_configs/rate_limit_local.rs:545-548,576`: interval only needs to parse; no minimum. Pinned `pingora-limits/src/rate.rs:143-178`: `maybe_reset` resets on every call when `reset_interval_ms == 0`, and the CAS-loser branch evaluates `assert!(new >= now - 1000)`, which underflows while `now < 1000 ms` after `Rate` creation (lazy per generation via `get_rate`, `plugin.rs:109-115`).
- Why lower: the boundary burst is the standard fixed-window property (Kong's default limiter has it); the 0 ms and sub-second hazards need an unusual operator config, and the panic additionally needs a contended CAS during the first second of a generation. Real correctness/doc bugs with a cheap fix (interval floor + doc/sliding estimate), but not a P1.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `rate_limit_local/plugin.rs`, `plugin_configs/rate_limit_local.rs` and `edgion-resources/src/duration.rs` are unchanged.
- No rate-limit review change touches the local limiter: only `graphql-query-cost-budgets.md` and `redis-script-cache-lifecycle.md` changed.
