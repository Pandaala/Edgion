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
admission. Subscription execution, caches, REST conversion,
ES DSL and SQL are outside the cost feature. Operation routing is a separate
HTTPRoute-only GraphqlOperationRouter plugin sharing the canonical bounded
frontend in edgion-resources/src/graphql_request and Gateway common/graphql_request. The cost is a trusted-chain snapshot:
operators must put authentication and all request rewrites before the plugin,
and later plugins/route/backend filters must not change the admitted operation.
There is no final-forwarding mutation seal. Entry conditions and policy masks
can skip the entire plugin; this is an explicit deployment boundary.

## Rationale and evidence

- [Supported configuration and cost semantics](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/graphql-rate-limit.md)
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

The additional SDL and Astra reviews identified P2 ineffective leaf multipliers:
scalar/enum return fields, including list wrappers, have zero child cost, so
the approved formula cannot charge their pagination amount. The shared compiler
now rejects nonempty multipliers unless the return named type is composite
(object/interface/union), using the same predicate as evaluation. Scalar/enum
resolvers require conservative fixed weights; no per-item leaf charge is implied.
Review changes to this check, wrappers, interface rule applicability or the formula
against both leaf rejection and retained composite/fixed-weight charging tests.

The pinned Apollo0.8.6/Rowan0.16.1 parser allocation ledger uses lexical preflight,
4096 native tokens,256-byte nontrivia tokens,64 native recursion, bounded JSON and
model cardinalities,32768 evaluation work steps and4 process-wide nonwaiting
permits shared by the router and limiter. Frontend selection and cost evaluation
carry one work counter without resetting it; parsed variables and CST metadata
are borrowed from one owner. The conservative96MiB per evaluation excludes shared BodyStorage,
compiled schema generations and allocator-wide RSS. Parser upgrades, cloning
variables/CST subtrees, retaining error diagnostics, collecting expanded fragments
or moving permit release across Redis require renewed resource review.

Schema/rule/representation mutations, executable extension support, downstream
query sealing and new budget transaction promises require independent review.
Multi-policy debit is individually atomic, and retries/backend failure do not
refund the early request charge. Missing all key sources denies; partial-missing
composition and4096-byte key truncation retain canonical limiter semantics.

## Verification

The original task design, review and implementation artifacts are no longer
present in the current tree. The results below are retained historical summaries,
not fresh verification; their original commands and report locations are unavailable
from those artifacts. Maintained regression coverage lives in the
[cost-model tests](../../../../edgion-resources/tests/unit/graphql_cost/mod_test.rs),
[Gateway admission tests](../../../../edgion-gateway/tests/unit/plugins/http/graphql_rate_limit/plugin_test.rs)
and [dynamic integration scenario](../../../../edgion-tests/integration/conf/EdgionPlugins/GraphqlRateLimit/DynamicTest/README.md).

The P2 leaf-multiplier correction passed fresh independent plan and final reviews
on 2026-10-07. The rejection/configuration tests failed before the guard and passed
after it. Current focused results: Resources14, Gateway4 and harness23 tests;
workspace check, warning-free final Clippy, format and project guards passed.
Canonical real-Redis integration passed35 static cases and all6 dynamic phases,
including leaf-invalid replacement (Accepted=False and fixed500) and recovery
at cost16. Final metrics/unknown-field/requeue/residue gates passed. No full Cargo
tests ran, and this correction does not establish a new parser heap proof.

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
reviews both returned IssuesFound for the subsequently corrected leaf configuration;
both freshly passed 11 Resources GraphQL tests, but neither independently
proved the complete parser allocation ledger or measured peak heap.
The four-request concurrent case isolates Redis atomic charging within the
independent four-permit parsing admission; parser exhaustion is covered by the
separate unit test. Final review evidence, exact commands and integration report
locations were recorded in the original task artifacts noted above.
Full cargo test --all was not run.

Two independent specification reviews passed on 2026-10-07 after concrete fixes to
GET allocation order, abstract rule maxima, partial key-miss semantics and POST
query ambiguity. This design evidence does not claim implementation test success.

## Resolution references

- 11-graphql-support: feature scope narrowed by user to Kong-like query-cost
  budgets on 2026-10-07. Operation routing remains outside the implemented
  feature scope. Its former pending task record is no longer present in the
  current tree; this does not establish implementation or dismissal.
- P2 ineffective leaf multipliers: diagnosed by two independent additional
  reviews; corrected in the existing shared compiler without changing the cost
  formula. Correction verification is summarized in the historical results above;
  historical feature checks do not establish that this boundary was covered.
