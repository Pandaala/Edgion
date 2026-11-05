---
name: shared-cumulative-bucket-retention
description: Review cumulative AI quota shared-bucket expiration, independent marker cleanup, and paused-stream accounting.
knowledge_type: regression-guards
---

# Shared cumulative buckets preserve active-marker retention

## Conclusion and scope

The canonical cumulative quota Lua script preserves or extends aggregate expiration
atomically. A shorter final, duplicate, stale, or conflicting request update cannot
shorten the shared bucket's existing TTL. Request markers still receive their own
independent bounded TTL. This covers business token quotas, not credential reservations.

`QuotaTarget.bucket_key` identifies the shared usage counter for the entry, identity,
optional logical model, and frozen request-start window. `request_key` identifies a
single request's revision, cumulative target, and final state. The aggregate must
survive while valid contributing markers rely on its already acknowledged charges.

## Rationale and evidence

ISSUE-004 exposed the following sequence: A flushes 900 with a long active lease;
B finalizes 10 with a shorter TTL and overwrites shared expiration. After the bucket
expires but before A's independent lease refresh, A resumes at cumulative 1000.
The script formerly computed `0 + (1000 - 900) = 100` instead of 1010 and returned
successful enforcement evidence. This is lost acknowledged usage, distinct from
the accepted cumulative-mode concurrency overshoot.

Sources:

- `edgion-gateway/src/ai/quota/runtime.rs`: `build_targets` freezes the window;
  `RedisQuotaSink::apply_all` selects active/final marker retention and requires
  the requested bucket TTL to cover it. `project_apply_result` tests `used > limit`.
- `edgion-gateway/src/ai/quota/mod.rs`: `run_worker` skips unchanged ordinary
  updates and independently refreshes charged active leases.
- `edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs`:
  `CUMULATIVE_QUOTA_SCRIPT::refresh_expiry` compares PTTL within the same atomic
  script before extending the shared bucket. Negative PTTL on a newly created
  hash receives a finite expiry. No Redis 7-only expiration option is needed.
- The [canonical Redis boundary](../../../01-architecture/00-common/05-unification-points.md#6-redis-access--unified)
  requires using the shared script executor and LinkSys client.

## Alternatives and limits

The user approved retaining longer shared expiry rather than adding an active-request
registry. Each aggregate remains one counter; no body copy, additional key, request
registry, extra network round trip, or production retry is introduced. One PTTL
command is added inside the existing script.

The aggregate can outlive all its contributing requests. Its last retained expiry
is finite, but there is no fleet-wide memory cap: cardinality, limit count, windows,
request durations, and request rate determine retained memory. Active updates may
continue extending old request-start windows. Marker expiry still ends idempotency.
Redis eviction, manual key deletion, or independent data loss are outside this
expiration correction; surviving markers cannot reconstruct other requests' charges.

Envoy references are comparison evidence, not authority for this ledger:
[envoyproxy/ratelimit](https://github.com/envoyproxy/ratelimit/blob/main/src/redis/fixed_cache_impl.go)
increments and expires counters without this paired cumulative-marker scheme;
[RLQS](https://www.envoyproxy.io/docs/envoy/latest/api-v3/service/rate_limit_quota/v3/rlqs.proto)
distinguishes assignment expiration from abandoning a bucket and reports request
counts. Neither establishes equivalent AI streaming settlement semantics.

## Verification and re-evaluation

The old script was reproduced on an exclusively owned Redis 7.4.11 instance: B's final
update reduced shared PTTL to approximately 935 ms while A's marker retained
approximately 29836 ms. This was a direct script reproduction, not an HTTP test.

The ignored integration target `edgion-tests/tests/cumulative_quota_expiry_redis.rs`
uses the production LinkSys client and shared executor. It covers active/final
updates, early-return refresh branches, zero-delta extension, idle intervals past
short retention, resumed aggregate 1010 and denied admission, separate-window
admission, conservative final watermarks, signed final reconciliation, and finite
expiry of a fresh short bucket. The old build failed at the assertion that B must
not shorten A's lease. The corrected build passed (1 test, about 1.25 seconds) on
the isolated Redis 7.4.11 instance through LinkSys. The final test revision also
passed after adding decreasing-target conflict coverage.

Fresh checks passed:

- `cargo fmt --all` and `git diff --check`.
- `cargo check --workspace --all-targets --locked`.
- `cargo clippy --workspace --all-targets --locked` (warnings in unchanged files).
- `cargo test -p edgion-gateway --lib quota --locked`: 60 passed, including sparse
  active lease heartbeats, unknown finalization, signed reconciliation, script
  contract/result parsing, and quota enforcement controls.
- Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway API transform contract, and unit-test layout guards.
- Independent read-only review returned LGTM after documentation and branch
  coverage corrections.

No full HTTP/SSE, hot-reload, Cluster, failover, memory benchmark, or full workspace
test result is inferred. The fix changes script expiry only; configuration snapshots
and hot-reload ownership are unchanged. Re-evaluate on changes to marker lifetime,
window selection, lease refresh, script refresh branches, or Redis eviction policy.

## Resolution references

- ISSUE-004 (review baseline `e6cbd068`): confirmed shared-bucket expiry defect; correction and verification
  are scoped to preserving acknowledged cumulative charges across TTL updates.
