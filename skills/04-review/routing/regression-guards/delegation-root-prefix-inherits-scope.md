---
name: delegation-root-prefix-inherits-scope
description: HTTPRoute delegation scope inheritance and ARCH-156 runtime ancestor path conjunction, including regex boundaries and internal transport validation.
status: fixed
finding_id: new_work-09-delegation-root-prefix
closed: 2026-07-29
knowledge_type: regression-guards
---

# Delegated Child `PathPrefix /` Inherits the Scope Prefix — FIXED

## Conclusion

In `flatten_route`'s `merge_match` (`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_delegation.rs`), a child match whose path is absent **or** equal to the Gateway API structural default `{type: PathPrefix, value: "/"}` inherits the delegated scope prefix. It is NOT treated as a literal root path and dropped by the containment check.

## Why

`edgion-resources` backfills an omitted `HTTPRouteMatch.path` with `{PathPrefix, "/"}` at deserialization (`default_path_prefix_root`, added 2026-07-28) so that a spec-valid method/header-only match compiles into a reachable route on the File/Etcd backends, which never pass through K8s admission defaulting. Side effect: by the time a child rule reaches the delegation flattener, "path omitted" is indistinguishable from an explicit root prefix — the flattener's original `None` branch ("inherit the scope prefix") became dead code, and every method/header-only child match was dropped with "outside every delegated prefix". The regression sat red in `child_match_without_path_inherits_prefix` because the backfill commit touched resources + gateway but not the controller's delegation module, and the program had been verifying Edgion with check/clippy only.

Inside a delegated scope, `PathPrefix /` — written or backfilled — can only mean "everything delegated to me": the child cannot exceed its scope by construction, and the user doc already promises the inheritance (`docs/en/user-guide/http-route/delegation.md`: "A child match without a path inherits the delegated prefix").

## Rejected alternatives

- **Drop explicit `PathPrefix /` but inherit omitted paths**: impossible — the structural default erases the distinction before the flattener runs.
- **Remove the structural default instead**: it exists to keep method-only matches reachable on non-K8s backends; reverting it reintroduces silently unreachable routes.
- **Fix the test to expect the drop**: contradicts both the module's documented design ("child-first-else-inherited") and the user documentation.

## Re-evaluation triggers

- `default_path_prefix_root` is removed or changed in `edgion-resources/src/resources/http_route.rs`.
- Delegation grows per-scope opt-outs for prefix inheritance.
- `Exact "/"` handling changes (today it stays a literal root path and is correctly dropped when out of scope).

## Known residual inconsistencies (pre-existing, deliberately not widened into this fix)

- A **delegating** child rule (one that forwards to a further HTTPRoute) whose
  match omits `path` still derives the scope prefix `"/"` in
  `validate_delegating_rule` and is dropped as out-of-scope under any non-root
  parent scope. Only **terminal** rules got the inherit semantics here. Extending
  inheritance to nested delegation is a behavioral decision for its own change.
- `is_root_prefix_match` tests the raw value before path normalization, so a
  pathological `PathPrefix "/./"` (which normalizes to `/`) is treated as a
  literal path, not as the structural default. Same behavior as before this fix.

## Reference

- Fix: `is_root_prefix_match` + the second `Some(p)` arm in `merge_match`.
- Pinning test: `route_delegation::tests::child_match_without_path_inherits_prefix`.

## Resolution references

- `new_work-09-delegation-root-prefix` — 2026-07-29.

## ARCH-156: semantic ancestor boundaries

The prior Controller regex check examined textual prefix adjacency, which did
not establish containment: `/api` plus `^/api/?secret$` admitted `/apisecret`,
and `^/api` admitted `/apix`. The prior anchor/alternation restrictions were
not a safe regex subset and are removed. Supported regex patterns now intersect
with every ancestor prefix at runtime; entirely disjoint patterns are valid but
never match. Ordinary regex routing is unchanged.

Controller flattening carries tagged `resolvedParentMatch` on effective rules:
`direct` for root siblings; `delegated` with a scope entry for each child match.
Ancestor path lists are conjunctive and capped by the existing eight-edge depth;
non-path predicates remain merged. For terminal regex matches, incompatible
parent OR branches are skipped independently; one conflicting branch must not
discard another compatible branch. If any child match has no compatible scope,
the rule is still dropped. Terminal Exact/Prefix conflicts still reject the whole
rule. Intermediate delegation skips incompatible parent/own pairs, but every own
match must retain at least one compatible parent scope or the intermediate rule
is dropped. Authored sidecars are cleared/replaced.

Ancestor prefixes are BASE-normalized once by the Controller and compiled directly
by Gateway. BASE normalization is not idempotent: `/%%32%66` becomes `/%2f`,
then `/` if decoded again. Repeating it on the sidecar widens the parent boundary.
Controller flattening/serialization and Gateway preparse/compilation regressions
pin this contract, with `/%%34%31` as a second overlapping-escape case. Method,
header and query conflict regressions cover both parent branch orders and the
all-incompatible outcome. These corrections address the additional ARCH-156 review.
Gateway compilation rejects absent sidecars on resolved rules and invalid scope
counts/content before plugin factories. It compiles canonical radix guards once,
then shared `deep_match` applies them before selecting a winner. A failure tries
the next candidate in both RegexSet and linear fallback, and in radix routing.
No post-selection 404 masks another valid candidate.

The canonical matcher preserves `/:param`, `/::`, prefix trailing-slash and
normalization behavior. Exact/Prefix static containment remains conservative;
this fix does not add symbolic subset checking or nested default-root inheritance.
A full bounded ancestor chain avoids relying on semantic subsumption of parameter
patterns. All guard data belongs to the same immutable unit generation. There is
no request-time resource lookup, new registry, regex parser, or duplicated filter
execution. Up to eight IDs fit the matcher result inline capacity eight and bound
the DFS frontier below its inline capacity sixteen.

Regression coverage resides in the existing resource transport, Controller
flattening, Gateway rule/engine tests and RouteDelegationDynamic scenario. The
scenario retains child `/pay` to `/pay2` recovery, adds optional-slash regex escape
probes, narrows only the parent, restores it, and verifies withdrawal on cleanup.
Fresh commands/results are recorded by the implementing task; historical results
above do not validate ARCH-156. Raw malformed JSON/unknown enum tags remain at the
existing serde error boundary, distinct from typed-valid rule withdrawal.

Re-evaluate when effective match ordering, ancestor depth, radix parameter grammar,
normalization, or the resolved-rule wire boundary changes. Preserve explicit
provenance and reject missing required scope metadata rather than treating it as
an ordinary match. Stable finding: `ARCH-156`.
