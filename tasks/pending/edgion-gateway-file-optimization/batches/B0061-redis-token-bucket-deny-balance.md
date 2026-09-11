# B0061 — Redis Token-Bucket Deny Balance

## Scope
- Baseline: `e15676a9457b6a0149acef8c7af0f98eb043babb`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/common/mod.rs`; `edgion-gateway/src/plugins/common/redis_rate_limiter/{mod.rs,scripts.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, RedisLinkClient/shared crates, resource schemas, consumers, metrics/docs, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation file; campaign ledger/changelog/main record; B0060 and B0061 batch records; reviewer-approved numeric-domain and integration-coverage tasks plus campaign deferred backlinks; legacy follow-up index

## Context and value gate
- Analysis subagent: `b0055_runtime_analysis`, revalidated against `e15676a9`
- Baseline: shared Redis rate-limiter tests pass 20/20.
- Concrete bug: the token-bucket deny branch computes a partially refilled balance and advances `last_ts`, but persists only the timestamp. With weighted cost greater than one, sub-cost refills are repeatedly discarded and a client can remain denied indefinitely despite elapsed refill time.
- Decision: persist both the computed token balance and timestamp on deny; add a source-contract regression proving both allow and deny paths write both fields.
- Expected no-change files: `plugins/common/mod.rs` retains correct entry-scope compilation and identity separation; the rate-limiter facade retains the canonical RedisLinkClient, one checked wall-clock sample, typed reply parsing, opaque same-slot AI keys, and safe consumer boundaries.
- Preserved behavior: Lua reply shape, TTL, EVALSHA/EVAL fallback, key/hash-slot semantics, weighted cost, clocks, fail policy, logs, metrics, and all caller-visible headers/outcomes.

## Deferred candidates
- Accepted `u64` rates can exceed Lua's exact integer domain and local checked-multiplication bounds, which can round or turn replies into fail-policy errors; this requires one shared numeric-domain contract across HTTP, stream, resources, Gateway defense, schema/docs, and tests.
- Existing integration tests remain frozen. A separate new ignored real-Redis scenario is warranted to prove that two one-token partial refills accumulate across a deny and later allow a weighted request, using explicit timestamps and no sleeps.

## Implementation
- Implementer: `b0056_implementation`
- Complexity: moderate but local; no plan-first design required
- Plan: write the computed balance and timestamp in the deny branch, then add a no-network source-contract regression covering both outcome branches.
- Actual change: the TokenBucket deny branch now persists both the computed partially refilled balance and timestamp; the colocated source-contract regression robustly isolates the outer allow and deny arms and requires both writes in each arm plus exactly two total persistence sites.

## Independent review
- Reviewer: `b0056_final_review`
- Verdict: Approved after one test-only robustness correction, with no residual findings
- Correction: anchored branch extraction to the exact outer allow return/else and deny return/end so a future nested conditional cannot false-pass the persistence contract.
- Deferred adjudication: D0028 records the nonduplicate shared numeric-domain task; D0029 records the new ignored real-Redis coverage requested for the fixed partial-refill behavior; existing integration tests remain frozen.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline core tests | passed | 20 tests |
| Implementation and focused tests | passed | 21 core, 11 HTTP, and 8 stream tests |
| Gateway all-target compile | passed | implementation verification |
| Independent code review | passed after correction | core tests, formatting, and diff checks passed |
| Full campaign gates | passed | format check, workspace check, workspace Clippy, agent docs, SSA, metrics inventory, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | existing integration tests unchanged from the user-authorized baseline |

## Handoff
- Ledger/changelog updated: yes
- Commit: `695f0ba9`
