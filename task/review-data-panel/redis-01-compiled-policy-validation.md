# Redis rate-limit policy validation ownership

Status: deferred design decision. Source baseline: `d3c85ddea`.

## Evidence

In `edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs`, `check`
first runs `validate_rate_limit_batch`: it rejects an empty list, validates the
request cost/timestamp, and validates every policy before checking client health
or issuing any debit. Its loop validates each policy again to obtain integer
milliseconds. `eval_policy` then calls `validate_rate_limit_execution`, which
revalidates the rate/window and cost/timestamp. Thus each policy's numeric domain
is checked three times on a successful batch call. This is repeated bounded
arithmetic and Duration conversion, not repeated textual duration parsing or an
allocation benchmark. Reply validation is separate and remains necessary.

HTTP `RateLimitRedis::new` validates a cloned config and populates parsed
intervals at construction. Its schema `RateLimitPolicy` nevertheless exposes
public fields and an optional runtime interval; that field is skipped by serde
and schemars, so its presence is not an unforgeable validity proof. Stream
`ConnectionRateLimitPlugin::one_policy` builds a stack policy for each Redis
dimension. Its empty String does not allocate, and its algorithm clone does not
copy heap data. DSL `DslRateLimits::from_config` already validates and resolves
numeric windows at construction, but its builtin calls the public raw
`eval_policy` entry with dynamic cost/time. Existing external integration tests
also exercise that entry directly.

The canonical bounds live in `edgion-resources/src/redis_rate_limit.rs`. They
protect exact Lua arithmetic and Redis integer operations. Removing batch
preflight would allow an earlier policy to debit before a later malformed policy
is discovered. Removing validation from the raw entry would expose bypassed
inputs. Neither is a behavior-preserving shortcut.

## Decision alternatives

- Retain the current small checks while measuring CPU cost separately from Redis
  latency. No measured throughput improvement currently supports a larger API
  change.
- Retain per-call preflight and introduce a private validated execution path,
  avoiding repeated checks only after the entire batch has passed. Decide how
  that proof and computed milliseconds survive into execution without adding a
  per-request collection or an easy-to-misuse unchecked entry.
- Introduce an immutable compiled policy owner with private fields and a fallible
  constructor shared by HTTP, Stream and DSL. Freeze rate/window/algorithm at
  construction, while keeping request cost/time and dependency replies validated
  at runtime. This requires reviewing caller ownership and public raw-input
  boundaries; it is not permission for separate limiter implementations or an
  additional schema/transport field.

The unresolved tradeoff is the cost of the repeated checks versus new proof/API
machinery and its maintenance. This remains an open optimization candidate, not
an exemption from cold preparation rules. No runtime change is made by this
record; a selected implementation needs a separately reviewed IMPL scope.

## Acceptance for a future implementation

Preserve all-policy preflight before any Redis command, including malformed
later policies, missing compiled intervals, empty lists and invalid cost/time.
Preserve validation-versus-health error precedence and diagnostic text on raw
entries, UTF-8 key truncation, policy index/order, explicit timestamps, all-policy
execution within a quota, and the existing result merge. An earlier deny inside
one quota does not permit skipping its remaining policies; named quotas have a
separate first-denial boundary. Keep runtime reply validation and failure policy.

Exercise exact numeric limits and fractional/overflowing windows. Verify HTTP,
Stream and DSL construction/reload ownership and raw-entry invalid-input rejection
before IO. Retain the canonical scripts, client resolver, keys, deadline and exact
NOSCRIPT-only fallback. Run the existing script lifecycle suite only on an
exclusively owned Redis instance; do not infer Cluster/failover guarantees from
standalone results. Measure arithmetic work, allocations and retained compiled
memory separately, without introducing per-request logs or metrics.

Relevant original knowledge:
- `skills/04-review/rate-limit/regression-guards/redis-named-quotas.md`
- `skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md`
- `skills/04-review/rate-limit/regression-guards/token-bucket-deny-branch-persists-balance.md`

This documentation batch uses current source inspection, not fresh runtime tests
or benchmarks. Earlier test results do not prove a future validation refactor.
