---
name: local-allowance-redis-overflow
description: Review local Continue admission, same-resource rateLimitExceeded dependencies, and Redis supplemental quota.
knowledge_type: regression-guards
---

# Local allowance and Redis overflow admission

## Conclusion and scope

RateLimitLocal.config.onLimitExceeded defaults to Reject. Continue lets local excess
reach later filters without a local rejection or excess quota headers. The shared
Condition rateLimitExceeded references an earlier active Continue RateLimitLocal by
pluginAlias, only within the same EdgionPlugins requestPlugins array and only in
a request RateLimitRedis entry's run conditions. Local passes skip that Redis
consumer; excess requests use its existing admission and failure policy.

## Rationale and evidence

The user selected this feature under source ID rate-limit-plus to avoid Redis IO
for locally admitted traffic. The earlier proposal to run Redis after every local
pass was explicitly superseded, as was creating a new RateLimitPlus plugin type.
The shared Condition ownership rule remains authoritative; do not add a second
trigger engine. Shared validate::collect_rate_limit_dependency_errors and the
placement profile preserve Controller/Gateway error parity and invalid-resource
sealing. Gateway compiled conditions bind aliases to numeric producer slots.

runtime/conditions/rate_limit_local_outcome.rs owns the typed outcome/frame mechanism;
ctx.rs stores its sparse request data. Both PluginRuntime and ExtensionRef request
dispatch establish and restore invocation frames. Without the ExtensionRef seam,
ordinary route references would never establish the state. A reused context or
same-named alias in another resource cannot provide a previous outcome.

NotEvaluated is distinct from WithinLimit: missing producer execution or missing
key Allow without fallback fails an active consumer closed with generic 500.
CompiledPluginConditions checks all dependencies before skip/allOf/anyOf
short-circuiting. Existing compile-time unresolved-reference policy is not enough:
that path can become Skip, while this dependency is missing at request execution.

## Alternatives and limits

The user excluded handling an operator disabling Redis. Disabled consumers remain
inert. No watchdog forces a Redis consumer to execute; additional valid conditions
may intentionally skip it. Continue is an explicit non-reject action, not a
guarantee of end-of-chain enforcement. Invalid active configuration still seals
the resource. The new predicate has no negate or cross-resource reference mode.

CMS still counts attempts, including excess; Continue does not refund or replace
the algorithm. With default Instance scope, replicas each contribute local
allowance; Cluster scope retains its existing approximate threshold calculation.
Redis supplements rather than caps local admissions. Redis entry/policy identity,
Lua refill, sequential accounting, and onRedisFailure default Allow are unchanged.
Examples explicitly choose Deny. Do not claim a strict global ceiling, durable
billing, or exact sliding-window bound from a token-bucket capacity.

## Verification and re-evaluation

Current implementation verification: resource overflow tests (5), existing
condition tests (27), Gateway runtime tests (364), request-state tests (7), and
the real Redis integration test (1) passed. The Redis test lives in
edgion-gateway/tests/rate_limit_local_overflow_redis.rs and uses the standard Redis
fixture; network IO stays outside library unit tests. Workspace all-target check
and Clippy passed (Clippy retains existing warnings), as did formatting,
document consistency, SSA, metrics, and Pingora-source guards.

The EdgionPlugins conditions subtree is structural even though plugin config
preserves unknown fields. Both direct and nested atomic schemas must declare
rateLimitExceeded and pluginAlias. Missing declarations caused an actual
Controller schema rejection before the CRD was corrected. Structural CRD tests
(14) and condition-specific guards (7) passed after correction. The complete
CRD guard failed its pre-existing KeyGet count/traceId drift during that
verification. The remaining cardinality issue is tracked in the separate pending
KeyGet traceId/CRD drift task.
Do not report that whole guard as passing without fresh verification.

RateLimitLocalOverflowDynamic passed against the current Controller/Gateway binaries:
absent, initial local allowance/Redis Deny, invalid-reference 500, repaired alias,
and final absence. Local execution used isolated temporary ports and the existing
loopback-upstream override for its local backend; the registered scenario itself
uses the shared integration topology and starts no processes.

Re-evaluate if condition placement widens, a new plugin execution entry bypasses
frame handling, condition short-circuit semantics change, or a global combined
quota is requested. Review request-local storage isolation and field ownership
whenever the context or mock/session adapter is reorganized.

## Resolution references

- rate-limit-plus: implemented and independently reviewed; verification above.
- edgion-gateway/src/plugins/runtime/conditions/rate_limit_local_outcome.rs
- edgion-gateway/src/plugins/runtime/conditions/evaluator.rs
- edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs
- edgion-resources/src/resources/edgion_plugins/validate.rs
