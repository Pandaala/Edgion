# GraphQL operation-name deduplication storage

Status: deferred optimization decision for the user. Baseline: `21b4a6215`.
No runtime change or measured performance improvement.

## Evidence

Gateway GraphQL request plugins enter the canonical Resources frontend through
common/graphql_request::evaluate. In
`edgion-resources/src/graphql_request/selection.rs::select`, a temporary
HashSet<String> detects duplicate named operation definitions. Every named
definition converts n.text() to String before insertion, including the duplicate
that triggers rejection. The set is not retained in ParsedGraphqlRequest.

Locked Apollo 0.8.6 Name::text returns TokenText holding an owned GreenToken
handle. TokenText compares/orders by text but has no Hash implementation. It is
not a drop-in HashSet key. Borrowing from the temporary text handle requires a
valid longer-lived owner; syntax/token identity must not replace textual equality.
Retaining TokenText retains its green token, not necessarily the entire CST.

The same selection helper serves cold safelist registration. GraphQL names here
come from each request, so ordinary configuration precompilation cannot eliminate
this runtime deduplication. Existing fragment/variable maps have separate retained
ownership and are outside this candidate.

## Decision and tradeoffs

Keep the current String keys, use a narrowly scoped key retaining TokenText with
textual Hash/Eq, or evaluate another bounded deduplication structure. None is
selected. A wrapper adds maintenance and token ownership/refcount costs; changing
the container changes lookup, allocation and adversarial-input costs. Avoiding a
String copy alone does not establish a faster or smaller complete evaluation.
Do not add a dependency or duplicate parser simply to expose a key representation.

This is within one frontend invocation, distinct from
[graphql-01](graphql-01-repeated-frontend-parsing.md)'s retained frontend reuse and
[graphql-02](graphql-02-prepared-type-conditions.md)'s prepared type lookup. It
authorizes no AST/context cache or process-wide operation-name interning.

## Acceptance for a selected implementation

- Preserve case-sensitive whole-document duplicate detection, including names
  repeated across query/mutation definitions and unselected operations. Keep
  anonymous-operation ambiguity, selector verification and GET mutation refusal.
- Preserve early rejection and directive validation order, every frontend Work
  charge, graph validation, token/name/envelope/recursion bounds and parser error
  mapping. Do not defer duplicate checks to a later pass without proving the same
  acceptance and failure precedence. No limit relaxation or input normalization.
- Require byte-consistent textual Hash/Eq if using a key wrapper. Equal names at
  different syntax positions must collide as names; case variants must remain
  distinct. Keep the existing hash-flood protection or separately review any
  container/hash policy change. Preserve all malformed/duplicate cases.
- Trace key ownership through early error and successful return. Temporary state
  must stay within evaluation admission; preserve panic/cancellation cleanup and
  the selected operation, fragments and variable owners. Cold safelist compilation
  must use the same semantics. Freeze an exact source/test allowlist.
- Measure complete parsing with one operation, multiple unique names, maximum
  accepted name lengths and near-bound documents, plus early/late duplicates.
  Compare allocation calls/bytes, retained storage, token refcounts and CPU cost;
  report common-case regressions as well as removed copies. Validate exact Work
  boundaries and errors against the original implementation.
- Require independent IMPL plan/final review and repository checks. Keep permits,
  body ownership, logs and cache proof behavior unchanged; no new instrumentation.

Fresh baseline verification passed 19 Resources graphql_request tests and 44
GraphQL-selected Gateway tests (63 total). They verify current behavior, not a
replacement representation. No prototype or benchmark ran. Relevant original
contracts: [static safelist](../../skills/04-review/rate-limit/regression-guards/graphql-static-safelist.md)
and [query-cost budgets](../../skills/04-review/rate-limit/regression-guards/graphql-query-cost-budgets.md).
