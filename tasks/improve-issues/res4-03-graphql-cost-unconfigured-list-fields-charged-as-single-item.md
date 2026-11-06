# res4-03: GraphQL cost model charges every unconfigured list field as a single item, so the budget is fail-open unless operators enumerate every list field

- **Category**: best-practice
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: local
- **Location**:
  - `edgion-resources/src/graphql_cost/request.rs:330-367` (`Evaluation::field`: `cost.map(Ok).unwrap_or_else(|| self.add(1, children))`)
  - `edgion-resources/src/graphql_cost/schema.rs:455-530` (`CompiledGraphqlCost::compile`: rules are optional, no list default)
  - `edgion-resources/src/graphql_request/input_type.rs:6-60` (`TypeRef.nulls` already records list wrappers)
  - `docs/en/user-guide/http-route/filters/edgion-plugins/graphql-rate-limit.md:42-45` ("An unconfigured field costs one plus its children")

## Summary

`GraphqlRateLimit` estimates `weight + product(multipliers) * sum(children)` only for fields that have an
explicit `costRules` entry. Every other field - including fields whose SDL type is a list of objects
(`friends: [User!]!`, `orders: [Order]`) - is charged `1 + children`, i.e. as if the list always contained
exactly one element. The schema compiler already knows which fields are lists, but nothing uses that, and
there is no "assumed list size" default. A query that nests unconfigured list fields (bounded only by
`maxDepth` <= 32) can therefore have an estimated cost of a few dozen units while the real resolver work is
`N^depth`.

## Current behavior (evidence)

- `Evaluation::field` (`request.rs:330-367`): if no candidate field has a `rule_index`, `charge =
  self.add(1, children)`; `children` is the max over possible concrete types of the summed child costs
  (`set`, `request.rs:151-178`). List cardinality never enters the formula.
- `Schema::compile` (`schema.rs:207-402`) parses `TypeRef` for every field; `TypeRef.nulls.len() > 1`
  identifies list wrappers (`input_type.rs:27-52`), but no rule or default is derived from it.
- `CompiledGraphqlCost::compile` (`schema.rs:459-530`) accepts any subset of fields in `rules` (max 256) and
  never warns about uncovered list-returning fields.
- Example (no rules): `{ users { friends { friends { friends { id } } } } }` costs 1 + 1 + 1 + 1 + 1 + 1 = 6
  units; with 100 users and 100 friends each the backend materialises ~10^8 objects. Raising `maxCost`
  cannot help because the estimate itself is wrong.

## Industry comparison

- IBM GraphQL Cost Directives spec / Apollo Router demand control: list fields without `@listSize` are
  charged with a mandatory operator-supplied default (`demand_control.strategy.static_estimated.list_size`
  in Apollo Router; `defaultListSize` in the IBM spec), and child cost is multiplied by it.
- Hot Chocolate cost analysis applies a default list size (and paging-argument awareness) to every list
  field unless overridden.
- Shopify/GitHub public cost models charge connections by `first`/`last` with a required maximum.

## Impact

The plugin is documented as a schema-aware query-cost budget, but its protection depends on the operator
having written a rule for every list-returning field. Missing one (common as schemas evolve) silently turns
the budget off for any query reaching that field, which is precisely the nested-list amplification the
feature exists to stop. The 400/429 behavior and `X-Graphql-Query-Cost` header give operators false
confidence because the reported cost looks small.

## Suggested direction

- Add a required-with-default `defaultListSize` (e.g. 10-50, bounded like `defaultMultiplier`) to
  `GraphqlRateLimitConfig`; in `Evaluation::field`, when no rule matches and the declared return type is a
  list of a composite type, charge `1 + defaultListSize * children` (and multiply per nested list wrapper).
- Keep explicit `costRules` (and multipliers bound to `first`/`limit`) as overrides, matching the
  `@listSize` model.
- Optionally emit a compile-time warning listing list-returning composite fields that have no rule.

## Notes

- Checked `skills/04-review/rate-limit/regression-guards/graphql-query-cost-budgets.md`: it records the
  rejection of multipliers on scalar/enum leaves and states that "cost units do not measure actual resolver
  CPU", but contains no decision about list cardinality for unconfigured fields; the user guide only states
  the current formula. No `tasks/` entry tracks it.

## Verification (independent)

- **Verdict**: DOWNGRADED (P2 -> P3)
- Code claim holds: `graphql_cost/request.rs:366` charges `self.add(1, children)` when no candidate has a `rule_index`; list cardinality is not consulted for unconfigured fields.
- Downgrade reason: this is the documented contract (`graphql-rate-limit.md:42-45`: "An unconfigured field costs one plus its children"; the guide also states weights and pagination defaults are operator estimates), so it is an unsafe default rather than a defect. Independent shape bounds (`graphql_guard.rs:23-33`: `maxDepth` 1..=32, `maxAliases`, `maxRootFields`, `maxFields`) also limit nested amplification when configured.
- A `defaultListSize` (Apollo Router `static_estimated.list_size`, IBM cost spec) is still a worthwhile hardening; keep as P3 improvement.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `edgion-resources/src/graphql_cost/`, `graphql_request/` and the user doc are unchanged; unconfigured fields still cost `1 + children`.
- `rate-limit/regression-guards/graphql-query-cost-budgets.md` gained only an Apollo 0.8.7 parser resource review; nothing new on list-field default cost.
