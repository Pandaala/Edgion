---
id: ARCH-053
status: candidate
kind: architecture-debt
priority: P3
partition: PART-193
analyst: /root/analyze_conversion
baseline: BASELINE-AUDIT-20261010-C.json
---

# ARCH-053: Preserve cold-resolved GraphQL cost-rule identity in the compiled field model

Kind: architecture-debt
Priority: P3
Status: candidate; independent verification pending
Scope: edgion-resources/src/graphql_cost/{mod.rs,request.rs,schema.rs}
Analyst: /root/analyze_conversion

## Current ownership and evidence

CompiledGraphqlCost owns the private SDL schema and a HashMap<String, GraphqlCostRule>
keyed by operator coordinates such as "Query.users"
(edgion-resources/src/graphql_cost/mod.rs:13–19).

During construction, compile_inner already splits each rule coordinate, resolves its
object/interface and actual schema Field, and validates its multiplier arguments,
return type and concrete defaults
(edgion-resources/src/graphql_cost/schema.rs:505–537). It then discards that resolved
field association and clones the rule into the coordinate-keyed map at line 539.

On the request path, Evaluation::field first resolves the static declaration and
actual concrete field. After calculating children, it iterates the concrete type
and its declared interfaces, charges one Work step per candidate, allocates
format!("{coordinate}.{n}"), and looks up that newly allocated String
(edgion-resources/src/graphql_cost/request.rs:262–266,288–311).

This allocation also occurs when costRules is empty or the candidate has no rule.
For example, the accepted SDL "type Query { name:String }", no rules, and query
"{name}" construct "Query.name" solely to miss the rule map. Repeated aliases,
fragment occurrences and applicable concrete/interface candidates repeat this
work. This is a source-confirmed temporary allocation; no latency, throughput,
peak-heap or production-incident claim is made.

The production owner is real rather than a test-only convenience.
GraphqlRateLimit::create builds and retains one immutable CompiledGraphqlCost.
Its request filter invokes estimate_quotas through the canonical shared GraphQL
evaluation owner, receives a Copy scalar report, checks capacities, and then
calls RateLimitRedis::run_request_with_costs. GraphqlGuard's optional SDL cost
ceiling also consumes this same pure compiled cost model. Operation routing and
safelisting consume the shared frontend but do not require this rule lookup.

## Why this boundary is avoidable

Keeping operator-facing coordinate strings is legitimate for typed configuration,
cold validation and duplicate detection. Conservative enumeration of applicable
interface and concrete rules is also required. Neither requires reconstructing
the configuration coordinate for each request occurrence after the field
association has already been established during construction.

The current representation makes request execution rebuild a cold configuration
identity. A change to rule applicability must therefore reason about both the
cold coordinate resolution and the hot coordinate formatting/map lookup. Keeping
the validated association in the immutable field model removes that repeated
identity reconstruction while preserving the required applicability traversal.

## Bounded improvement

Retain one bounded owned rule collection in CompiledGraphqlCost and attach an
optional compact, checked rule index to each existing private SDL Field during
cold compilation. Resolve and validate operator coordinates with the current
strict compiler, retaining duplicate rejection and existing diagnostics.

During evaluation, obtain the rule index from the actual concrete field and,
in the existing order, from each declared interface's matching field when
present. An interface that does not declare this concrete field remains a miss.
Borrow the rule from the single compiled owner. Do not copy an interface rule
onto every concrete implementation or precompute expanded applicability matrices.

This removes rule-coordinate String construction, including for the empty-rule
default. The private field/model representation is not serialized and does not
derive JsonSchema, so this need not change CRD, serde transport or public plugin
configuration. Existing bounds permit at most 256 rules and a globally bounded
SDL field model; the optional field slots add a small bounded cost to each compiled
generation. Record that tradeoff explicitly. It is separate from the existing
per-evaluation source estimate and does not establish a process-wide schema-memory
or allocator RSS guarantee.

The change should remain within the three Resources cost-model files and relevant
pure regression coverage. Gateway lifetime, admission, body handling, Redis
enforcement and HTTP routing contracts need no new API or ownership layer.

## Alternative

A two-level map supporting borrowed type and field lookup can also avoid coordinate
formatting, with additional cold map ownership instead of per-field indices.
Compare its bounded generation cost before choosing the representation.

An empty-rule fast path is smaller but only addresses the default configuration;
sparse configured models would retain repeated miss allocations. Any such path
must still preserve the existing Work accounting, including concrete/interface
misses. No query AST cache, mutable shared memo, runtime registry, generic dispatch
framework or expanded rule matrix is justified by this finding.

## Required invariants and migration risks

Preserve one Work step for every current concrete/interface candidate, in the
same order, even when no rule applies. Do not reset the frontend Work counter,
skip misses, or increase the accepted work envelope as a side effect of reducing
allocation. Preserve multiplier Work steps, recursion limits, checked arithmetic,
maxCost rejection and business depth limits.

For each concrete candidate, retain the maximum complete applicable charge
(weight + multiplier product * child cost), rather than selecting the first rule
or independently maximizing weight and factor. Selection cost must still take
the maximum across possible concrete objects. Interface rules continue to use
the actual concrete field's argument defaults. Static fragment-validity context
and covariant runtime possibilities remain distinct.

Preserve literal and variable semantics: zero is floored to one, negative or
invalid multipliers reject, undefined variables differ from explicit null,
required/default handling remains unchanged, and supported integral JSON numeric
representations remain supported. Leaf multipliers remain rejected during cold
validation; composite multipliers and fixed leaf weights remain chargeable.

Keep __typename's existing early scalar charge and shape restrictions, and reject
other unsupported introspection fields. Repeated aliases, fragment spreads and
skip/include branches remain conservatively counted.

Root quotas retain their fixed 15-slot Copy report, complete depth-1 charges,
conservative repeated occurrences and total-only base operation charge. Capacity
checks, absent-root behavior, canonical Redis ordering/failure policy and completed
numeric cost visibility remain unchanged.

The bounded JSON/CST owner, four process-wide nonwaiting evaluation permits and
request BodyStorage remain canonical. Parsed state and the evaluation permit
must still be released before Redis awaits. Preserve fixed redacted errors and
bounded numeric PluginLog/header output. No query text, variables, coordinates or
new traffic tracing should be added to request logs.

A stale or incorrectly attached index could silently omit a rule, alter interface
precedence or use the wrong concrete defaults. Cold validation and runtime lookup
must therefore migrate together, with indices checked against their immutable
generation owner and no unsafe indexing shortcut.

## Original decisions and distinguishing verification

The original graphql-query-cost-budgets review requires maximum complete
interface/concrete charges, concrete defaults, one monotonic Work budget and
release before Redis. Its parser ledger is a conservative source estimate, not a
measured heap cap, and excludes compiled generations. The current source and
regression bodies support these requirements; historical test summaries are not
fresh execution evidence.

Relevant current Resources test bodies include:
- abstract_maximum_and_concrete_default_cannot_be_bypassed: concrete default 7,
  interface/concrete whole-charge maxima, and crossed weight/factor result 203.
- covariance_preserves_static_fragment_validity_and_runtime_applicability.
- aliases_defaults_and_nested_multipliers and
  variables_preserve_undefined_null_and_integer_semantics.
- leaf_multipliers_are_rejected_but_fixed_weights_remain_chargeable.
- root_quotas_use_complete_weighted_maxima_and_conservative_occurrences,
  root_coordinates_support_custom_shared_query_mutation_objects, and
  root_evaluation_does_not_reset_frontend_work.

Gateway admission bodies cover bounded early rejection, permit release on failure,
panic and success, completed estimate visibility before dependency/capacity
outcomes, and root-capacity checks before debit. Guard tests cover optional cost
measurement and hard-failure behavior without reflecting input.

Future implementation verification should retain these behavioral regressions
and examine empty, sparse, concrete and interface rule lookups. Check that rule
lookup itself no longer constructs owned coordinates, without claiming that the
entire GraphQL frontend becomes allocation-free. A distinguishing allocation
measurement may quantify the benefit, but must separate rule lookup from existing
name, argument, JSON and parser allocations. Revalidate near-exhausted Work
behavior so the representation change does not relax admission.

Repository findings were searched for GraphQL ownership overlap; no existing
GraphQL finding was found. ARCH-050's repeated cold validation invocation is a
different root cause and is not included here. Original task artifacts referenced
by the GraphQL knowledge page are absent, as that page itself records; their
historical acceptance does not authorize implementation or substitute for current
evidence.

## Limits

This is a bounded request-path ownership optimization, not a demonstrated
incorrect-cost defect. Expected allocation reduction follows directly from the
current format! site; its performance magnitude remains unmeasured. No tests,
builds or distinguishing runtime experiments were executed during this review.
