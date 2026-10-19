# Prepared GraphQL type-condition lookup

Status: deferred user decision. Source baseline: `82add4ca3`.

## Evidence and scope

Gateway GraphqlGuard optional costing and GraphqlRateLimit enter the canonical
Resources cost visitor through shared bounded GraphQL evaluation. In
`edgion-resources/src/graphql_cost/request.rs`, `Evaluation::condition` checks
static type overlap and concrete-object applicability. Static validation and
runtime fragment evaluation can repeat the same immutable type-pair scan; runtime
selection also visits each possible concrete object to retain conservative maxima.

The condition loop visits every target possible type and scans source possible
types until the first match. It does not exit when overlap first becomes true.
Each target visit and each source comparison consumes one carried Work step.
For source length N and target length M, the complete charge is:

`M + sum(first matching source position, counted from 1; N when absent)`.

That complete charge and overlap depend on the ordered source/target pair, not
the current object. Applicability separately tests current-object membership in
the target. The target uses a checked lookup and composite-kind gate before the
loop; source is indexed under context established by callers. Empty/disjoint
pairs retain their existing rejection. A budget can expire before the complete
scan, so successful-charge equivalence alone does not establish failure behavior.

`edgion-resources/src/graphql_cost/schema.rs` owns possible-type vectors. Object
vectors contain themselves; unions retain their compiled member order; interface
membership is collected while traversing the type HashMap. Sorting or rebuilding
these vectors can change first-match positions, Work thresholds and traversal
order. The compiler bounds total types at 256 including built-in scalars, and
bounds declared memberships. The runtime Work cap remains 32768 across frontend,
optional guard shape inspection and costing, not a fresh allowance per lookup.

## Alternatives for user decision

- Retain bounded scans and measure realistic object/interface/union shapes, repeated
  fragments and near-budget requests before adding persistent indexes.
- Investigate bounded same-generation type IDs, target membership bitsets or source
  position indexes. Such indexes may avoid repeated string comparisons without
  requiring an all-pairs table, but still need exact original Work accounting.
- Evaluate a prepared pair table containing overlap and complete ordered-scan
  charge, with separate object membership. An all-pairs representation scales
  quadratically with the type cap; bitsets, sparse tables and lazy strategies have
  different memory, publication and request-path costs. No layout or winner is
  selected by this record.

Simple membership/intersection replacement silently reduces charged Work and
changes which documents are accepted. A bulk-charge primitive would need an
independently reviewed equivalence argument for exact-limit and over-limit paths,
including the existing failed-step counter behavior. Keeping one Work.step call
per old comparison preserves accounting but may retain much of the CPU cost.
Precomputation must not consume additional existing compiler Work and thereby
reject previously accepted models without explicit authorization; bound new build
work independently while retaining current validation semantics.

## Acceptance before implementation

Preserve static-versus-runtime type context, complete applicable-rule maxima,
concrete defaults, root quotas, directives, recursion, unknown/noncomposite target
rejection and disjoint applicability. Exercise empty, overlapping, disjoint and
reordered possibility vectors; object/interface/union pairs; covariance; repeated
fragments; and exact remaining Work for successful and failing evaluations.
Preserve per-generation vector order rather than assuming reproducible HashMap
iteration across compilations. IDs and indexes must be owned and published by the
same immutable cost model as the schema and rules, with no cross-generation reuse.

Measure compilation time, retained bytes per generation, multiple live generations
and request CPU/allocation cost at small and near-cap model sizes. The existing
conservative 96 MiB per-evaluation estimate excludes compiled schema generations;
it is not authorization for additional persistent storage. No context AST cache,
new body owner, expanded fragment graph, global lookup cache or request-path lock
is authorized here. Keep canonical nonwaiting admission and parser/permit release
before Redis, fixed failures, redaction and access-log ownership unchanged.

Relevant original knowledge:
- `skills/04-review/rate-limit/regression-guards/graphql-query-cost-budgets.md`
- `skills/04-review/rate-limit/regression-guards/graphql-guard-selected-shape.md`

No runtime implementation or benchmark accompanies this record. Existing focused
tests are baseline evidence only; fresh execution results are recorded in INDEX.
