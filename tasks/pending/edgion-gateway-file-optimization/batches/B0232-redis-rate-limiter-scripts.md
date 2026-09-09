# B0232 — Redis Rate Limiter Scripts

## Scope

- Pipeline state: ready to commit; implementation, independent final review and all gates passed.
- Active anchor: `edgion-gateway`.
- Baseline: external `55bb2c84`; target blob unchanged since analyst refresh at `774c63bc`.
- Target: `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs`, reopened after external `264847df`; historical B0061 remains evidence, not current coverage.
- Inspect the complete target, canonical shared Redis rate-limiter core, all Gateway callers, script semantics and tests. Determine change, examined-no-change or defer with measurable evidence.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Do not promote before B0230 and B0231 complete their serial commit gates.

## Analysis Outcome

- Analyst `b0232_redis_scripts_analysis` recommends change with plan-first complexity at external HEAD `774c63bc`; target blob `add839921301c21d0caf52800fdc51a9a9077f19`. External `264847df` only corrected EVALSHA lifecycle commentary and is consistent with the current descriptor/fallback implementation.
- Confirmed candidate: integer refill resets `last_ts = now` and permanently discards `elapsed * refill_rate % interval_ms`. For rate 3/1000 ms with requests every 500 ms, a depleted bucket refills 4 instead of 6 tokens over two seconds; loss can approach 50% for rate 2 with 999 ms steps. Both allow and deny paths are affected. This is distinct from B0061 deny-path persistence and D0028 numeric-bound work.
- Proposed invariant: persist a per-key fractional refill remainder, default missing state to zero, accumulate it atomically, clear it while saturated, never move `last_ts` backward, account for it in retry time and persist state on both branches. Preserve single-key Lua atomicity, TTL/return shape, shared RedisLinkClient, EVALSHA fallback and caller failure policies.
- Exact candidate scope: target, `skills/04-review/rate-limit/regression-guards/token-bucket-deny-branch-persists-balance.md`, new standalone integration-gap task `tasks/todo/add-redis-token-bucket-fractional-refill-integration-coverage.md`, and coordinator records. Existing integrations remain immutable. Implementer must return a no-edit plan for independent review before any technical edit.
- Suggested filters: scripts tests, `redis_rate_limiter`, `rate_limit_redis`, `connection_rate_limit`. Original plugin/LinkSys/Redis architecture, unified executor, failure-policy, B0061/script-cache knowledge, observability and testing rules were independently applied.

## Approved Plan

- Implementer proposed three-field `tokens`/`ts`/`refill_remainder` state with missing remainder defaulting to zero. Clock rollback preserves timestamp/remainder; one full interval saturates safely under the enforced `refill_rate == capacity` invariant; sub-interval refill carries quotient/modulo remainder and clears it when the pre-consumption balance reaches capacity.
- Both allow and deny persist all three fields. Deny retry uses positive required numerator and overflow-avoiding integer ceil `floor((required - 1) / refill_rate) + 1`. Preserve TTL, four-integer reply, key/API, descriptor/EVALSHA fallback and caller failure policy.
- Tests use independent fixed expected outcomes for cadence, migration, saturation, rollback and retry boundaries plus source anchors; they explicitly do not claim Lua execution. Add the standalone real-Redis coverage task without changing existing integrations. D0028 retains broader HTTP/Stream numeric-domain ownership.
- Independent plan reviewer `b0232_plan_review`: APPROVE at `55bb2c84`, no scope expansion. Any new production path, reply/key/TTL/failure change, shared numeric validation or integration edit requires renewed review.
- External Transport/ClientHello provenance and new Gateway test inventory: [reconciliation](../external-sync-55bb2c84.md).

## Implementation and Review

- Implementer `b0232_implementation` followed the approved three-path plan: lazy two-field migration, carried fractional refill, rollback preservation, full-interval saturation, capacity clearing, fractional retry calculation and three-field persistence on both outcomes. Redis key, one-key atomicity, TTL, four-integer reply, descriptor/EVALSHA fallback and failure policies are unchanged.
- Added fixed-expectation, network-free tests for cadence, migration, saturation, rollback, large elapsed values, both branches and retry rounding. Updated owning knowledge and created the self-contained future real-Redis task `tasks/todo/add-redis-token-bucket-fractional-refill-integration-coverage.md`; existing integrations are untouched.
- Fresh independent reviewer `b0232_final_review`: LGTM with no Must Fix or Should Fix at external `55bb2c84`. Reviewed source blob `590bbea526f6b67da47f0f1676cdb34d0f059840`, knowledge blob `54ab1876a1ce489b2ba7a538c8727374d1aebe18` and task blob `9930b466eec53e0f8721d4ac3cd15f381749770a`; confirmed numeric exactness, migration and preserved contracts.
- Fresh tests: scripts 4/4, shared limiter 27/27, HTTP Redis limiter 14/14 and connection limiter 12/12. Workspace format/check/Clippy, agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards passed.
- Commit: `e42437af`; never pushed by the campaign.
