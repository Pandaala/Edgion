# Redis fallback command ownership

Status: deferred ownership and performance decision for the user.
Source baseline: `fe08228fe`. No runtime or dependency change.

## Current evidence

The shared `RedisRateLimiter::eval_script` in
`edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs` receives an owned
MultipleKeys and Vec<Value>. It wraps the arguments in Value::Array, clones both
containers for EVALSHA, and retains the originals across await. Only an Unknown
Fred error whose first ASCII-whitespace-delimited token is exactly NOSCRIPT
permits one EVAL fallback with the retained inputs.

All six call sites use one or two keys and three, four, five or seven integer
arguments. The earlier direct-array preparation changes removed intermediate
vectors before this point; they did not remove these fallback snapshots.
Fred 10.1.0 derives Clone for MultipleKeys and Value. Cloning their Vec containers
copies key handles or integer values. Key wraps Bytes, so cloning it shares byte
ownership rather than unconditionally copying key payloads; sharing may itself
allocate owner metadata. This evidence does not establish an exact allocation
count or a throughput benefit for a replacement.

Fred's `commands/interfaces/lua.rs` accepts inputs by value.
`commands/impls/lua.rs` consumes them into the command-building closure, checks
the key slot and constructs its command vector. The API returns the result or
error, not the original inputs. Simply moving the current snapshots into EVALSHA
would leave no inputs for EVAL.

Fred's `types/scripts.rs::Script::evalsha_with_reload` is not an equivalent
replacement. It uses SCRIPT LOAD followed by another EVALSHA and tests an error
prefix with starts_with. Edgion's existing exact classifier, single EVAL fallback
and absence of remote preloading remain authoritative.

## Alternatives for a separate decision

- Retain the current snapshots and their straightforward cancellation ownership.
- Retain an immutable description of the command inputs and construct fallback
  containers only after the exact cache-miss result. Choose owned or borrowed
  key storage and validated scalar storage explicitly; do not introduce a
  parallel script executor or generic retry mechanism.

The second option could avoid eager container duplication on the warm path,
but may retain different capacities, enlarge suspended futures, add byte copying
on cache misses or change reference-count work. Raw eval_policy callers supply
their own strings; HTTP key truncation does not establish a global key-length
bound. Measure actual key lengths and capacities rather than assuming small keys.
No recipe representation, helper signature, closure or dependency change is
selected by this task.

## Acceptance for an approved implementation

Freeze the exact script, keys, argument order and validated integer values for
the whole operation. Never recompute time, window ID, cost, quota revision,
cumulative target, TTL, policy generation or credential identity on fallback.
Keep the captured LinkSys client and existing pool routing; do not refresh the
client or introduce a new timeout window. Preserve duplicate quota-admission
keys and the same Redis Cluster hash tags.

Keep all validation before execution and all error/result mappings. Only the
existing exact NOSCRIPT classification permits one EVAL. Timeout, disconnect,
permission errors, embedded NOSCRIPT text and ambiguous execution do not permit
application replay. Preserve concurrent cold-operation semantics, cancellation
cleanup and conservative accounting after uncertain execution. Do not claim
exactly-once transport behavior or transactional multi-policy admission.

Compare warm success, genuine cache loss, fallback failure and non-cache-miss
errors. Measure preparation allocations, copied bytes, retained capacity, future
size and cancellation release separately. Exercise every script family and both
key arities without changing the existing integration fixtures. Run the committed
`edgion-tests/tests/redis_script_lifecycle.rs` only against an exclusively owned
Redis instance: SCRIPT FLUSH affects the server, not just a logical database.
Standalone results alone do not prove Cluster or failover behavior.

Keep the canonical shared executor, existing access-log ownership, sensitive-data
handling and observability limits. Add no request tracing, diagnostic key payloads
or new metrics to justify this optimization.

## Original knowledge and verification limits

- [Script cache lifecycle](../../skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md)
- [Named quota ordering and partial debits](../../skills/04-review/rate-limit/regression-guards/redis-named-quotas.md)
- [Separate compiled-policy validation decision](redis-01-compiled-policy-validation.md)

This task records source evidence and an unresolved tradeoff. Current tests
validate the existing implementation, not a proposed lazy fallback design.
No live Redis test, allocation probe or throughput benchmark ran for this audit.
