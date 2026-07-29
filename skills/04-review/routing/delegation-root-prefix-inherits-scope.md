---
name: delegation-root-prefix-inherits-scope
description: Use when reviewing HTTPRoute delegation flattening; covers why a child match with the structural-default PathPrefix "/" inherits the delegated scope prefix instead of being dropped as out-of-scope.
status: fixed
finding_id: new_work-09-delegation-root-prefix
closed: 2026-07-29
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
