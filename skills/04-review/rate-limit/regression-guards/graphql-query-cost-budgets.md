---
name: graphql-query-cost-budgets
description: Review GraphqlRateLimit parsing, conservative cost rules, bounded admission and shared Redis enforcement.
knowledge_type: regression-guards
---

# GraphQL query-cost budgets

## Conclusion and scope

GraphqlRateLimitConfig owns a static SDL cost model and explicit Redis budgets.
The Resources pure compiler serves Controller validation and Gateway compilation;
the Gateway does not trust Controller acceptance. Costs flow through
RateLimitRedis::run_request_with_cost and its canonical RedisRateLimiter/key
composition, entry scope, timeout, headers and failure policy. GraphQL defaults
Redis failure to Deny without changing ordinary RateLimitRedis's accepted Allow.

The supported scope is early global/Gateway/route request-stage query/mutation
admission. Subscription execution, operation routing, caches, REST conversion,
ES DSL and SQL are outside the feature. The cost is a trusted-chain snapshot:
operators must put authentication and all request rewrites before the plugin,
and later plugins/route/backend filters must not change the admitted operation.
There is no final-forwarding mutation seal. Entry conditions and policy masks
can skip the entire plugin; this is an explicit deployment boundary.

## Rationale and evidence

- [Specification and resource ledger](../../../../tasks/pending/graphql-cost-rate-limit/01-design.md)
- [Pure model](../../../../edgion-resources/src/graphql_cost/mod.rs)
- [Typed configuration](../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/graphql_rate_limit.rs)
- [Gateway admission](../../../../edgion-gateway/src/plugins/http/graphql_rate_limit/plugin.rs)
- [Canonical weighted limiter](../../../../edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs)
- [Body rules](../../../../AGENTS.md) and
  [lazy materialization](../../http-body/regression-guards/request-body-gate-lazy-materialization.md)

GET borrows the current URI query and bounds encoded bytes before copying;
POST rejects a nonempty URI query. Strict duplicate detection and bounded JSON
parsing precede GraphQL parsing. No query/JSON/CST state survives the Redis await.
The canonical request BodyStorage owns capture; response streaming is unaffected.

Each concrete object applies the maximum complete applicable interface/concrete
field charge, using concrete argument defaults. Selection cost takes the maximum
across possible concrete objects. Covariant runtime possibilities remain separate
from static fragment-validity context. These rules prevent changing a fragment's
spelling from selecting a cheaper rule. Repeated selections and skip/include
branches are conservatively counted; cost units do not measure actual resolver CPU.

## Limits and re-evaluation

Additional independent SDL and Astra reviews on 2026-10-07 confirmed one open
P2: nonempty multipliers are accepted on scalar/enum return fields, including
list wrappers, but their zero child cost makes the charge independent of the
multiplier. Do not infer pagination protection from successful validation of
these rules. A conservative fixed weight works under the current formula;
the pending correction must reject ineffective multipliers and clarify usage
guidance without silently changing cost semantics. See the
[open finding](../../../../tasks/pending/graphql-cost-rate-limit/05-issues.md).

The pinned Apollo0.8.6/Rowan0.16.1 parser allocation ledger uses lexical preflight,
4096 native tokens,256-byte nontrivia tokens,64 native recursion, bounded JSON and
model cardinalities,32768 evaluation work steps and4 process-wide nonwaiting
permits. The conservative96MiB per evaluation excludes shared BodyStorage,
compiled schema generations and allocator-wide RSS. Parser upgrades, cloning
variables/CST subtrees, retaining error diagnostics, collecting expanded fragments
or moving permit release across Redis require renewed resource review.

Schema/rule/representation mutations, executable extension support, downstream
query sealing and new budget transaction promises require independent review.
Multi-policy debit is individually atomic, and retries/backend failure do not
refund the early request charge. Missing all key sources denies; partial-missing
composition and4096-byte key truncation retain canonical limiter semantics.

## Verification

Fresh implementation verification passed on 2026-10-07: 11 Resources GraphQL
tests, 3 Gateway GraphQL tests, 19 ordinary Redis limiter regressions, 23 harness
registration checks and 40 CRD/stage guards. Full workspace check and Clippy,
format, documentation and project guards passed. The real-Redis integration
runner passed 35 static cases and all four dynamic phases, including invalid
configuration replacement, recovery and changed costs. Final residue validation
found no retained resource handles. A trusted URI rewrite adds the current GET
query from an entry with no parameters; costing and exact upstream query replay
both pass, while duplicate GET parameters remain rejected.

Fresh independent implementation review passed with LGTM on 2026-10-07;
the reviewer verified original rules, current code, knowledge and fresh logs.
This is historical evidence. The subsequent user-requested SDL and Astra
reviews both returned IssuesFound for the leaf-multiplier configuration above;
both freshly passed 11 Resources GraphQL tests, but neither independently
proved the complete parser allocation ledger or measured peak heap.
The four-request concurrent case isolates Redis atomic charging within the
independent four-permit parsing admission; parser exhaustion is covered by the
separate unit test. Final review evidence is recorded in
[review evidence](../../../../tasks/pending/graphql-cost-rate-limit/02-review.md).
Full cargo test --all was not run. Exact commands and fresh integration report
location are in
[implementation verification](../../../../tasks/pending/graphql-cost-rate-limit/02-implementation.md).

Two independent specification reviews passed on 2026-10-07 after concrete fixes to
GET allocation order, abstract rule maxima, partial key-miss semantics and POST
query ambiguity. This design evidence does not claim implementation test success.

## Resolution references

- 11-graphql-support: feature scope narrowed by user to Kong-like query-cost
  budgets on 2026-10-07. Operation routing remains an explicit
  [pending requirement](../../../../tasks/pending/graphql-operation-routing.md);
  it is not claimed implemented or dismissed.
