---
name: radix-candidate-precedence-rank-precomputed
description: Use when reviewing findings about RadixRouteMatchEngine candidate ordering — why precedence is ranked once at build time, why RadixPath::precedence_rank is 0 until the engine finishes building, and which "optimizations" are already rejected.
status: fixed
finding_id: task-preorder-radix-route-candidates
closed: 2026-09-02
knowledge_type: regression-guards
---

# Radix route candidate precedence is ranked at build time — FIXED

## Conclusion

`RadixRouteMatchEngine` resolves Gateway API route precedence **once**, during
`initialize_internal` (`assign_precedence_ranks`), and stores the result as
`RadixPath::precedence_rank`. `match_route` therefore orders candidates with a plain
`sort_unstable()` over `(precedence_rank, path_idx)` integer pairs. The ordering
step consults no comparator, never compares `MatchSpecificity::route_key` (a
`String`), and never reaches into `self.routes`; only the deep-match loop that
follows resolves `route_idx` through `self.routes`.

Fix suggestions of the form "move the precedence comparison back into
`match_route` so `RadixPath` has no post-construction field", or "sort each
radix node's bucket independently and merge", are **not accepted**.

## Core Rationale

**1. The ordering inputs are immutable snapshot data**

The precedence chain reads exactly two things: `RadixPath::specificity`
(derived from the path pattern) and `HttpRouteRuleUnit::specificity` (method,
header count, query count, creation timestamp, route key, rule index, match
index). Both are frozen when the route snapshot is published through the
existing `ArcSwap` ownership model. Nothing in the chain depends on the request,
so evaluating it per request is repeated work with a request-independent answer.

**2. A strict total order makes the global rank exactly equivalent**

`cmp_candidate` is a lexicographic chain over fixed fields with `path_idx` as
the final tie-break, so it is a strict total order. Restricting a strict total
order to a subset yields that subset's order — therefore sorting a request's
candidate subset by the global rank produces exactly what a direct comparator
sort of the subset would produce. The claim is locked by
`conf_handler_impl::tests::engine_ranks_reproduce_direct_candidate_ordering`,
which builds a real engine from HTTPRoute fixtures and asserts full sequence
equality against a direct comparison of the current path key and non-path chain.
The named-path engine tests additionally assert explicit candidate sequences;
comparator equivalence alone cannot validate the path policy. The algebraic
guards in `radix_route_match.rs` —
`rank_order_never_inverts_the_direct_comparator`,
`cmp_candidate_is_a_strict_total_order`, and
`sample_candidates_exercises_every_precedence_level` — cover the comparator
itself; each states its own reach in its doc comment.

Historical context: the pre-ranking code used a stable `sort_by`, so fully-tied candidates kept radix
traversal order. Ties are now broken by `path_idx` (build order) instead, and
that preserved the **same sequence** for that optimization. The 2026-09-12
named-parameter repair intentionally changes path precedence; this historical
argument does not promise equivalence to the removed weight formula:

- A full tie requires equal path specificity **and** an identical
  `MatchSpecificity`, hence identical `(route_key, rule_index, match_index)` —
  which names one match item of one rule of one route.
- The same match item carries the same path string, so both tied candidates
  normalize to the same `tree_key` and land in the **same** bucket of
  `tree_value_paths`. Cross-bucket full ties cannot occur.
- Within a bucket, the old code pushed candidates in bucket order, which is
  ascending `path_idx` — exactly the new tie-break.

Note that a tie is between two *different* `route_idx` values, not the same one:
`HttpRouteRuleUnit::extract_paths` returns at most one path per unit, so
`path_idx -> route_idx` is injective and no two `RadixPath`s ever share a route.
A tie therefore means the same match item was registered twice, which a route
listing a duplicate hostname does today (`effective_hostnames` performs no
dedup). Those duplicates are content-identical, so the choice between them is
unobservable regardless. Were `extract_paths` ever to return several paths per
unit, cross-bucket ties would become reachable — but then the tied candidates
would share a `route_idx` and `deep_match` would return the same result, so the
ordering stays safe under either evolution.

**3. `precedence_rank` cannot be computed in `RadixPath::new`**

The rank of one path depends on every other path in the same snapshot, which is
not known until the build loop finishes. The field is documented as valid only
through a fully built engine.

The HTTP match-engine module is crate-private and its three child modules are
visible only within `routes::http`. This type-level boundary prevents external
callers from constructing a standalone `RadixPath`; outside `#[cfg(test)]`, the
only construction site is `initialize_internal` and the only read is
`ordered_candidates` (called from `match_route`), both behind the engine's
private `radix_paths`. The tests reach the field through the `#[cfg(test)]`
`paths_for_test` accessor.

Historically, the fully public module chain exposed `RadixPath` outside the
crate, leaving the ranked-only invariant as a convention while `0` doubled as
both "unassigned" and "highest precedence". That accidental API surface was
removed once consumer searches confirmed that all match-engine users belong to
`routes::http`. Splitting the struct into "pre-rank" and "ranked" types remains
disproportionate for a single internal construction site. Revisit that if
`RadixPath` ever gains a second construction site outside the engine.

**4. The bucket table is dense, so it is a `Vec`, not a `HashMap`**

Tree values are assigned by `initialize_internal` as a dense sequence starting
at 1, so `tree_value_paths: Vec<Vec<u32>>` indexed by `tree_value - 1` replaces
the `HashMap<u32, _>` hash lookup the old code did once per matched tree value.
The index uses `checked_sub(1)` plus `get`, so a value outside the built range
skips the bucket rather than panicking.

**Historical attribution (not rerun for the new path key).** A throwaway review microbenchmark over the two loops (real
SipHash map, real `cmp_precedence`, 80-byte `RadixPath`) — not committed, so the
figures below are indicative only and cannot be re-derived from the tree — put
the total saving at roughly 60 ns
per request at a realistic candidate count of 4, of which about half came from
removing the hash lookups rather than from the ordering change. The `String`
comparison in `cmp_precedence` was rarely reached even before: the chain
short-circuited on the former `priority_weight`. These figures do not establish
the size or performance of the current `PathSpecificity` representation.
Do not cite the `String` comparison as the primary motivation. Against a
multi-microsecond per-request budget this is ~0.1-1%: the change is justified as
strictly less work with no pessimization path, not by a throughput number. No
threshold was established — see `tasks/todo/radix-route-matching-benchmarks.md`.

## Fix Suggestions Not Accepted

- "Inline the precedence comparator back into `match_route`" — reintroduces
  per-request `String` comparison and double indirection for an answer that is
  fixed at build time.
- "Sort each radix node's candidate bucket at build time and skip the request
  sort" — per-node sorting does **not** preserve global order; candidates from
  different nodes still need a merge, and a k-way merge over tiny k is slower
  than one integer `sort_unstable`.
- "Cache the resolved route per request path" — a route-result cache is an
  explicit non-goal for this engine (unbounded key space, invalidation on every
  snapshot swap); the frozen radix plus build-time ranking already gives the
  request path bounded work.
- "Add a second router or a mutable request-time index" — breaks the immutable
  published-snapshot ownership model.

## Re-evaluation Triggers

Re-open this decision only if:

- Route precedence gains a request-dependent input (for example, a runtime
  weight or a per-client override), which would break the "immutable snapshot"
  premise the whole rank rests on.
- A snapshot can hold more than `u32::MAX` paths, overflowing `precedence_rank`
  / `path_idx`.
- Benchmarks (see `tasks/todo/radix-route-matching-benchmarks.md`) show the
  remaining candidate collection — not the ordering — dominates match cost.

## Reference Cases

- Task "Preorder Radix Route Candidates Outside the Request Hot Path"
  (`tasks/todo/preorder-radix-route-candidates.md`, since deleted), closed
  2026-09-02 with a narrowed scope: ordering only, benchmark suite split out to
  `tasks/todo/radix-route-matching-benchmarks.md`.
- `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs`
- `edgion-gateway/src/routes/http/match_engine/radix_path.rs`
- `edgion-gateway/src/routes/http/conf_handler_impl.rs` — the engine-level
  ordering, bucket, candidate-filter, and tied-candidate tests
- `skills/01-architecture/02-gateway/03-routes/SKILL.md`
- Sibling ordering rule: [radix-path-specificity.md](radix-path-specificity.md)

## Resolution references

- `task-preorder-radix-route-candidates` — 2026-09-02.
