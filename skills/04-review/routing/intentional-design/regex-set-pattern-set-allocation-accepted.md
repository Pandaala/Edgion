---
name: regex-set-pattern-set-allocation-accepted
description: Use when reviewing per-lookup PatternSet result allocation in the HTTP RegularExpression route engine (RegexRoutesEngine::match_route / match_child), including proposals to reuse matcher scratch or switch to a lower-level regex API.
status: wont-fix
finding_id: routing-05
closed: 2026-11-05
knowledge_type: intentional-design
---

# Multi-pattern RegexSet result allocation is accepted

## Conclusion and scope

`RegexRoutesEngine` (`edgion-gateway/src/routes/http/match_engine/regex_routes_engine.rs`)
keeps calling `RegexSet::matches(path)` for multi-pattern sets in `match_route` and for
every set in `match_child`. Each call allocates one short-lived `Box<[bool]>` sized to the
pattern count of that hostname group's or delegation subtree's regex set; it is dropped when the lookup
returns. This ephemeral allocation is an accepted cost. Proposals to remove it by
reusing a caller-owned result set or scratch buffer are not accepted on present
evidence.

The singleton case is already handled separately in `match_route`: a one-pattern set
uses `RegexSet::is_match` and constructs no result set. That optimization is a
separate, implemented decision and is not reopened here.

This is a no-evidence verdict, not a proof that reuse has no value. No candidate
implementation or comparative benchmark was run.

## Rationale and evidence

- Locked `regex` 1.13.1 `regexset/string.rs::matches_at` calls
  `PatternSet::new(self.meta.pattern_len())` on every call, then
  `which_overlapping_matches`. `regex-automata` 0.4.18 `util/search.rs::PatternSet::new`
  allocates `vec![false; capacity].into_boxed_slice()`.
- The hidden `read_matches_at` / `matches_read_at` caller-buffer API also constructs a
  `PatternSet` internally before copying into the caller slice, so switching to it does
  not remove the allocation.
- The allocation is N bytes (N = regex routes in one `RouteRules`, i.e. a hostname group
  or a delegation subtree). It is paid when N >= 2 in `match_route`, and for every
  non-empty set in `match_child`. The same call runs an overlapping search over the full path, whose cost is not
  expected to be dominated by one small allocation. No measurement shows otherwise.
- Candidate priority depends on the ascending index order of the set matches combined
  with the build-time longest-first route sort; a deep-match rejection
  (ancestor/listener/method/header/query) must advance to the next candidate, so the
  full ordered result set is required, not a single first hit.

## Alternatives and limits

Rejected alternatives and their costs:

- **Thread-local or shared scratch `PatternSet`** — `match_child` is reentrant: while
  iterating one set's matches, `descent::match_child_unit` -> `descend` ->
  `RouteRules::match_child` can call another subtree's `RegexRoutesEngine::match_child`.
  A single borrowed slot would conflict (`RefCell` panic or forced fallback), and a
  shared lock adds contention on the request path.
- **Request-context-owned scratch** — needs capacity growth/reset rules, retained idle
  memory at the maximum group size, and isolation between old and new route generations.
  No such owner exists today.
- **Lower-level `regex_automata::meta::Regex` with a reused `PatternSet`** — would
  replace the `RegexSet` wrapper, risking divergence in compilation defaults, size
  limits and cold set-compilation failure behavior (which currently falls back to
  linear per-route matching). Maintaining competing regex semantics is rejected.
- Replacing the result set with a first-hit search is incorrect for the reason above.

Accepted cost: one small short-lived heap allocation per multi-pattern lookup.

Limit: `match_child` does not have the singleton `is_match` shortcut. That is outside
this decision and is not claimed optimal or harmful here.

## Verification and re-evaluation

Source facts above were rechecked against the locked crate sources on 2026-11-05. No
runtime code changed, and no benchmark or new test was run for this decision.

Re-evaluate if:

- Profiling of route matching shows this allocation (or allocator contention from it)
  is a material share of matching CPU or allocation count, measured against the current
  singleton-optimized engine.
- `regex` exposes a public `RegexSet` API that accepts a caller-owned `PatternSet`
  without changing engine defaults.
- Route matching gains an existing per-request scratch owner that already handles
  reentrancy, capacity bounds and generation isolation.

## Resolution references

- `routing-05` — "Evaluate multi-pattern regex route result storage"
  (`task/review-data-panel/routing-05-regex-result-storage.md`, baseline `d75c4f1c8`),
  deferred as a P3 performance investigation; closed 2026-11-05 as an accepted
  tradeoff by user decision, without code change.
- Related ancestor-boundary guard used by regex child matching:
  [delegation-root-prefix-inherits-scope.md](../superseded/delegation-root-prefix-inherits-scope.md).
