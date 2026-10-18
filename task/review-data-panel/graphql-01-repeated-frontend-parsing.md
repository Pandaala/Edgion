# Repeated GraphQL frontend parsing across request plugins

Status: deferred design decision. Source baseline: `b38733da0`.

## Evidence

Four request plugins call the canonical
`edgion-gateway/src/plugins/http/common/graphql_request.rs::evaluate` separately:
`graphql_safelist/mod.rs`, `graphql_guard/mod.rs`,
`graphql_rate_limit/plugin.rs` and `graphql_operation_router/plugin.rs`.
Each executed call rechecks transport and its envelope limit, acquires one of
four nonwaiting process-wide evaluation permits, and parses the current GET query
or POST body. A chain containing multiple such plugins can therefore repeat
frontend work on unchanged input. Skipped or terminal plugins do not establish
that all four evaluations occur on every request. No CPU/heap benchmark was run.

`edgion-resources/src/graphql_request/mod.rs::ParsedGraphqlRequest` owns JSON,
selected CST metadata, variable/fragment maps and a Work counter. The computation
closure consumes this owner. Each evaluation carries its own frontend Work into
its business computation. The owner and permit are released before evaluate
returns; GraphqlRateLimit subsequently performs its canonical Redis debit.
The existing conservative 96 MiB per-evaluation estimate is not a measured heap
cap and excludes BodyStorage and compiled generations.

Successful evaluation publishes bounded response-cache proof for POST when a
body generation exists, then freezes request-body mutation. The freeze does not
freeze the current GET query, URI, method or headers. Per-plugin envelope limits,
conditions, policy masks, compiled generations and terminal decisions also differ.
A body-generation-only cache key is therefore not enough to prove reusable input
or reusable policy results. Existing BodyStorage materialization reuse is distinct
from retaining a parsed JSON/CST owner.

## Decision alternatives

- Keep independent bounded evaluations and first measure frontend CPU/allocation
  costs for realistic multi-plugin chains separately from Redis and upstream I/O.
- Investigate a separately reviewed bounded execution group for eligible adjacent
  inspections. Prove dispatch, conditions, logs, response headers, terminal order,
  cache-proof publication and freeze timing before claiming equivalence. Grouping
  cannot silently move a Redis debit or routing side effect across another plugin.
- A retained parsed owner would require an explicit new lifetime/admission design.
  Releasing its permit while retaining its memory invalidates the current resource
  rationale; retaining the permit across unrelated plugins or Redis changes busy
  behavior and contention. This task grants no permission for a context AST cache,
  a second body cache, a new global cache, or relaxed parsing/admission limits.

Even grouping synchronous inspections changes how many nonwaiting admissions
occur and how long permits are held. That may change which request receives 503.
This is a user decision and design boundary, not an automatic safe optimization.

## Acceptance for a future implementation

Preserve each evaluation's current frontend-plus-business Work acceptance limit.
Skipping the frontend charge on reuse would loosen it; combining the whole chain
under one cumulative Work counter would tighten it. Neither is equivalent.
Preserve transport/media/method checks, per-entry envelope caps, strict duplicate
handling, operation selection, all parser/recursion/token bounds, error precedence
and nonwaiting admission unless an explicit scope change authorizes otherwise.

Prove current-input and compiled-policy identity across rewrites, GET versus POST,
body generations, hot reload, conditions, bypass/block, measure mode and early
termination. Preserve proof publication and mutation-freeze timing, scalar-only
cost reports, exact quota debit order, routing authorization and fixed errors.
Exercise cancellation, panic and release on all exits; account for retained heap
and permit lifetime across any await. Do not introduce request tracing, metrics,
query reflection, response buffering or a second parser implementation.

Relevant original knowledge:
- `skills/04-review/rate-limit/regression-guards/graphql-query-cost-budgets.md`
- `skills/04-review/rate-limit/regression-guards/graphql-guard-selected-shape.md`
- `skills/04-review/rate-limit/regression-guards/graphql-static-safelist.md`
- `skills/04-review/routing/regression-guards/graphql-operation-router-declared-backends.md`

This record is based on current source inspection. Historical feature tests are
not fresh verification of reuse, and no runtime optimization is implemented here.
