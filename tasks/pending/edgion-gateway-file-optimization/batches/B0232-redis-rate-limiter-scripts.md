# B0232 — Redis Rate Limiter Scripts

## Scope

- Pipeline state: analysis-ready; queued, no mutation authorized.
- Active anchor: `edgion-gateway`.
- Baseline: `6e4af44e`.
- Target: `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs`, reopened after external `264847df`; historical B0061 remains evidence, not current coverage.
- Inspect the complete target, canonical shared Redis rate-limiter core, all Gateway callers, script semantics and tests. Determine change, examined-no-change or defer with measurable evidence.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Do not promote before B0230 and B0231 complete their serial commit gates.

## Analysis Outcome

- Analyst `b0232_redis_scripts_analysis` recommends change with plan-first complexity at external HEAD `774c63bc`; target blob `add839921301c21d0caf52800fdc51a9a9077f19`. External `264847df` only corrected EVALSHA lifecycle commentary and is consistent with the current descriptor/fallback implementation.
- Confirmed candidate: integer refill resets `last_ts = now` and permanently discards `elapsed * refill_rate % interval_ms`. For rate 3/1000 ms with requests every 500 ms, a depleted bucket refills 4 instead of 6 tokens over two seconds; loss can approach 50% for rate 2 with 999 ms steps. Both allow and deny paths are affected. This is distinct from B0061 deny-path persistence and D0028 numeric-bound work.
- Proposed invariant: persist a per-key fractional refill remainder, default missing state to zero, accumulate it atomically, clear it while saturated, never move `last_ts` backward, account for it in retry time and persist state on both branches. Preserve single-key Lua atomicity, TTL/return shape, shared RedisLinkClient, EVALSHA fallback and caller failure policies.
- Exact candidate scope: target, `skills/04-review/rate-limit/regression-guards/token-bucket-deny-branch-persists-balance.md`, new standalone integration-gap task `tasks/todo/add-redis-token-bucket-fractional-refill-integration-coverage.md`, and coordinator records. Existing integrations remain immutable. Implementer must return a no-edit plan for independent review before any technical edit.
- Suggested filters: scripts tests, `redis_rate_limiter`, `rate_limit_redis`, `connection_rate_limit`. Original plugin/LinkSys/Redis architecture, unified executor, failure-policy, B0061/script-cache knowledge, observability and testing rules were independently applied.
