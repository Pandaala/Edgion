# Resolve RouteDelegation Omitted-Path Default Semantics

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-07-28 |
| Status | pending |
| Type | bugfix / design correction |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Restore a coherent contract between the Gateway API structural default for an
omitted `HTTPRouteMatch.path` and Edgion's RouteDelegation rule that a child
match without a path inherits the delegated parent prefix. The current
deserialization default erases the omitted-versus-explicit distinction and
causes the existing delegation test to fail deterministically.

## Evidence

The final body-work validation ran:

```text
cargo test --all
```

The controller stopped with:

```text
conf_mgr::sync_runtime::resource_processor::handlers::route_delegation::tests::
child_match_without_path_inherits_prefix
```

A focused exact rerun failed identically. The child match contains only
`method: GET`, but `HTTPRouteMatch.path` is deserialized to
`PathPrefix("/")`. RouteDelegation therefore sees an explicit root path rather
than `None`, rejects it as outside the delegated `/api` prefix, and produces no
flattened rule.

The closed finding `gw-http-match-01` states that RouteDelegation's `None`
branches remain unaffected. That statement is incomplete: the code-built match
for a rule with no `matches` still uses `None`, but a deserialized match object
with method/header/query constraints and no path is now defaulted to `/`.

## Requirements

- Decide the canonical delegation meaning of an omitted child match path after
  Kubernetes structural defaulting:
  - inherit the delegated parent prefix, preserving the current delegation
    design and test; or
  - treat it as the Gateway API root path and reject it outside a non-root
    delegated prefix, updating the delegation contract and tests.
- Apply identical semantics across Kubernetes, File, and Etcd configuration
  centers.
- Do not reintroduce the original bug where ordinary header/method/query-only
  routes become unreachable outside delegation.
- Preserve the ability to distinguish or safely normalize explicit root-path
  behavior according to the selected contract.
- Update the `gw-http-match-01` closed-finding text so it no longer claims the
  affected branch is unreachable by this regression.

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-resources/src/resources/http_route.rs` | Match default representation |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_delegation.rs` | Delegated child containment and inheritance |
| `skills/04-review/CLOSED-FINDINGS.md` | Correct the stale closed-finding statement |

## Acceptance criteria

- The selected semantics are documented with the omitted and explicit `/`
  cases.
- The focused failing test passes or is deliberately replaced by tests for the
  selected contract.
- Tests cover a child rule with no `matches`, a match with method/header but no
  path, and an explicit root path.
- Kubernetes-shaped and direct File/Etcd deserialization inputs behave
  identically.
- `cargo test --all` proceeds past the current deterministic failure.
