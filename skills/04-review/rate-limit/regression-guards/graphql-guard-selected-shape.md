---
name: graphql-guard-selected-shape
description: Review independent GraphQL shape protection, conservative fragment counts and optional cost work ownership.
knowledge_type: regression-guards
---

# GraphQL guard selected-operation shape

GraphqlGuard uses canonical strict GraphQL GET/POST parsing and the process-wide
four-permit evaluation admission. BodyStorage remains the only request-body owner.
The pure Resources shape visitor counts the selected operation's field occurrences,
aliases and root fields, including reachable repeated fragments and all directive
branches. Semantic field depth differs from the independent traversal recursion
bound. No expanded AST or per-request collection is introduced.

Limits are optional: omitted values default; serialized null stays null across
conf_sync and disables only the business threshold. Zero count limits prohibit
the counted shape. Introspection names are checked before aliasing: __schema and
__type are rejected by default; __typename remains ordinary counted metadata.

Optional SDL maxCost reuses CompiledGraphqlCost with the same parsed Work after
shape traversal. Null maxDepth maps to the cost engine's existing supported
ceiling32, not default20 or unlimited depth. SDL/costRules without maxCost and
introspection-enabled costing are rejected during shared Controller/Gateway
compilation. The plugin introduces no Redis link, backend permission or publisher.

This is a trusted-chain inspection snapshot. All rewrites must precede it; later
plugins must not change the admitted operation. Conditions/policy masks can bypass
inspection. Backend full GraphQL validation and field authorization remain separate.

GraphQL HTTP rejections use static JSON error bodies and queue application/json
only on failure. ErrResponse runtime ownership reasserts actual Content-Length
after queued headers and removes Transfer-Encoding. Native gRPC terminal mapping
retains its own content type. GraphqlRateLimit selects the same static 429 body
through a cold RateLimitRedis override; ordinary limiter bodies and canonical
quota/Retry-After ownership remain unchanged. Generic failures outside plugin
execution retain their existing response contract. See the
[HTTP error contract](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/graphql-errors.md).

The delegated limiter's JSON selection appends application/json immediately before
its rejection header write. Setting it only on ResponseHeader is insufficient:
the real adapter drains earlier queued response-header operations afterward, and
an earlier Content-Type can otherwise mislabel the JSON body. Success and ordinary
limiter ordering must retain their existing semantics.

Optional rootFields.query/mutation compile bounded immutable sets. Missing/null
allow is unrestricted, empty allow denies every root, and deny wins. The verified
operation kind selects a policy. The same visitor checks underlying names only
at semantic field depth1, including fragment roots and conservative directive
branches. __typename is an ordinary policy-checked root. Introspection gating
precedes root membership; allow cannot enable __schema/__type. Root denial403
shares the fixed error contract, while shape/cost/introspection violations stay400.
No extra traversal, per-request set, nested authorization or cost math is added.
Whole-resource DSL/WAF retention may preserve prior permissions for a bundled
update containing invalid DSL/WAF; separate resources give independent updates.

See [configuration](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/graphql-guard.md),
[shape visitor](../../../../edgion-resources/src/graphql_request/guard.rs),
[config compiler](../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/graphql_guard.rs),
and [runtime](../../../../edgion-gateway/src/plugins/http/graphql_guard/mod.rs).

Re-evaluate parser upgrades, new fragment/count semantics, null serialization,
cost/shape Work separation, introspection changes and mutation sealing against
these invariants. Verification evidence belongs to the current task record;
this knowledge conclusion does not substitute for fresh tests or review.

## Numeric calibration boundary

GraphqlGuard mode defaults to enforce. Explicit measure relaxes only configured
numeric shape/cost business thresholds. The same bounded visitor returns complete
scalar counts; it retains checked counters, carried Work and recursion, and enforces
introspection/root permissions. Optional measure costing compiles under the existing
supported hard ceiling1,000,000/depth32 after validating the original maxCost.
Failure returns no report or saturated/exact-looking estimate. Enforce retains its
early short-circuit behavior. Other plugins and Redis quotas are unaffected.

Completed reports use existing bounded PluginLog numeric entries, with no query,
operation-name or variable reflection. showQueryCost opt-in queues the completed
estimate; Guard requires costing. Cost is an estimate, not a debit receipt. Shared
GraphqlRateLimit records the estimate before capacity/dependency outcomes and passes
the same scalar into the canonical weighted limiter; parsed owners/permits are
released first. No tracing, metrics, full-body owner or report cache is added.

Re-evaluate measurement changes against hard failure/no-estimate, permission, Work,
mode-hot-reload and canonical quota/header tests.

Guard optional SDL costing shares the canonical strict standard metadata compiler
with GraphqlRateLimit. Accepted deprecated/specifiedBy metadata grants no root
permission, cost exception or executable directive support.

## Canonical body admission freeze

Successful canonical GraphQL evaluation now calls the existing RequestBodyState
mutation freeze, preserving the inspected body and propagating refusal to a
subsequently installed bodyless replay owner. Guard measure mode also freezes
a successfully inspected body. Failed or skipped evaluation does not freeze.
Later body mutation through DSL/ProxyRewrite/Wasm or REST adaptation is refused;
multiple adapters before inspection remain allowed. Query/URI/method/header
changes still require the documented trusted-chain ordering. See the
[adapter/body ownership contract](../../http-body/regression-guards/rest-to-graphql-request-adaptation.md).
