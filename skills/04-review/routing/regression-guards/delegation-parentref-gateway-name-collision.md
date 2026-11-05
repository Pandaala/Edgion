---
name: delegation-parentref-gateway-name-collision
description: Use when reviewing Gateway parentRef filtering, delegation reverse locks, or same-coordinate Route and Gateway identities.
knowledge_type: regression-guards
finding_id: delegation-parentref-gateway-name-collision
---

# Delegation ParentRefs cannot attach to a same-coordinate Gateway

## Required behavior

HTTPRoute and GRPCRoute delegation child parentRefs (mandatory under the
cascading-match model; formerly an optional reverse lock) use the Gateway API
group with `kind: HTTPRoute` or `kind: GRPCRoute`. They are not Gateway
attachments even when a Gateway exists with the same namespace and name. The
Gateway must publish only parentRefs whose normalized identity is the Gateway
API group plus `kind: Gateway`; omitted group and kind retain the Gateway API
defaults.

`edgion-gateway/src/routes/common.rs::filter_accepted_parent_refs` enforces this
before both optimistic publication and status-backed publication. Its status
lookup compares normalized group and kind in addition to namespace, name,
section, and port, so an accepted delegation status cannot supply acceptance to
a colliding Gateway ref. Preserve the surviving Gateway-ref order.

## Regression coverage

The focused `routes::common::tests` cover exclusion with and without status,
same-coordinate status isolation, explicit/default Gateway equivalence, and
mixed-ref ordering.

The local dynamic scenarios
`DelegationParentRefGatewayCollisionHttp` and
`DelegationParentRefGatewayCollisionGrpc` add live Controller/Gateway evidence.
Each fixture deliberately gives the Gateway and delegating parent Route the same
namespace/name. The child has only a route-kind parentRef at those
coordinates. A wildcard listener admits both the parent's sole hostname and a
distinct direct-probe hostname, so a direct 404 or gRPC UNIMPLEMENTED result
cannot be explained by listener hostname rejection. The same child rule succeeds
through the delegating parent; the access-log `match_info` chain lists the parent
Route first and the child (leaf) last, and the scenario checks the leaf identity.

Both scenario registrations use
`<RouteKind>/Delegation/ParentRefGatewayCollision/DynamicTest` as `conf_path`
in local and Kubernetes modes. Both fixture trees place `initial/` and
`updates/` there, while `bootstrap/` stays beside `DynamicTest/` so startup
can publish the Gateway separately. The former local layout omitted
`DynamicTest/`, causing initial apply to fail; moving its route generations
also lets the static loader exclude update fixtures using its shared dynamic
directory contract. The client unit test
`registered_scenario_paths_resolve_lifecycle_fixtures` checks both topology
roots against each bootstrap, initial, and update fixture.

Both scenarios update only the child from explicit metadata generation 1 to 2.
Their Gateway lives in `bootstrap/` and must be published before Gateway process
startup because listener ports are not dynamically bound. The scenario never
deletes it and its absent phase verifies the exact generation-1 listener on both
Controller and Gateway. Each run deletes only its kind-qualified parent and child
Routes, waits for their absence, and applies its own `initial/` directory before
the update, so a broad static loader cannot preselect either generation. They
poll for exact parent status identity, `Accepted=True`, matching
`observedGeneration`, the current Gateway payload, removal of the old behavior,
activation of the new behavior, continued direct rejection, and cleanup. The
HTTP scenario changes `/delegated/v1` to `/delegated/v2`; the gRPC scenario
changes `SayHello` to `StreamNumbers` and checks their concrete backend payloads.
All resources use one namespace, so this guard does not change or make a claim
about ReferenceGrant authorization.

Historical verification on 2026-09-09 (not a rerun of the path correction above):

- `cargo test -p edgion-gateway --lib routes::common::tests`: 34 passed.
- `cargo test -p edgion-tests --bin test_client`: 21 passed, including dynamic
  scenario inventory and listener-contract checks.
- The existing HTTPRoute and GRPCRoute Delegation integration suites passed
  4/4 and 5/5 respectively.
- `DelegationParentRefGatewayCollisionHttp` and
  `DelegationParentRefGatewayCollisionGrpc` both passed against a shared local
  FS Controller, Gateway, and test-server topology. Each Gateway listener was
  loaded before Gateway startup; the scenario then owned only its parent and
  child Route lifecycle.

## Scope and re-evaluation

Fresh path-correction verification on 2026-09-10: the test-library suite passed
91 tests, including both fixture trees; the test-client binary passed 21 harness
checks. Both collision dynamic scenarios passed in the canonical default sweep
(`tmp/integration_testing/testing_20260910_154454/report.log`, 237 passing report
entries, zero failures/timeouts). Independent review returned LGTM after replacing
the initial local-only registration change with the shared fixture layout.
Kubernetes path contracts were checked, but live Kubernetes was not run.

This guard covers Gateway-side publication and live attachment behavior. The
Controller's broader canonical parent-status writer, retention, comparison, and
configured-controller ownership contract is covered by the
[canonical ownership guard](../../status/regression-guards/canonical-parent-status-ownership.md).

Re-evaluate this guard when changing parentRef defaulting, route delegation
reverse locks, status matching, HTTP/gRPC route publication, listener hostname
matching, or dynamic scenario inventory.

## Resolution references

- `delegation-parentref-gateway-name-collision`: production filtering fixed in
  commit `78cf23d9`; live HTTPRoute and GRPCRoute regression coverage added by
  the deferred testing task.
