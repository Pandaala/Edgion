---
name: store-stats-testing-router-naming
description: Use when reviewing Gateway/Controller findings of the form "/api/v1/debug/store-stats is registered in create_testing_router but its path uses /debug/ instead of /testing/" or "the store-stats endpoint is unreachable in production despite the /debug/ naming"; covers why the path prefix and the testing-mode gate are intentionally orthogonal, mirroring the F-004 testing-router boundary model.
---

# `/api/v1/debug/store-stats` path prefix vs testing-mode gate — ACCEPTED-TRADEOFF

## Conclusion

`/api/v1/debug/store-stats` being registered inside `create_testing_router`
(`src/core/gateway/api/mod.rs:513`, gated by `is_integration_testing_mode()` at
`mod.rs:224`) while its URL path uses the `/api/v1/debug/` prefix instead of the
`/api/v1/testing/` prefix shared by its sibling routes is an **accepted naming
choice under the same boundary model that closed F-004**, and is not patched in
isolation.

Fix suggestions of the form "promote `store-stats` into `create_admin_router` so
it is reachable in production" or "rename the path to
`/api/v1/testing/store-stats` to match sibling routes" are **not accepted** as
F-B01-scoped fixes.

## Core Rationale

**1. The path prefix and the testing-mode gate are intentionally orthogonal**

What makes a route a "test-fixture surface" in this codebase is its membership in
`create_testing_router` (and thus the `is_integration_testing_mode()` mount
gate), not its URL prefix. The boundary established by F-004 — see
`skills/04-review/architecture/testing-router-error-body-enumeration.md`
— treats the entire testing router as a single fixture surface. Conflating the
prefix with the gate would invert that model: a future reader could see a path
beginning with `/api/v1/debug/` or `/api/v1/testing/` and incorrectly infer the
mount condition from the name, instead of consulting the router topology.

`/api/v1/debug/store-stats` is genuinely a debug/diagnostic endpoint — it dumps
a `StoreStatsResponse` of ~26 top-level derived-store counters (20+ subsystem
counters per the issue write-up) consumed by the `cache_residue` validator and
the `run_conf_sync_*.sh` leak-detection scripts. The `/debug/` prefix describes
its *function*; the testing-mode gate describes its *availability*. The two
dimensions are deliberately decoupled.

**2. The endpoint is a converged Gateway+Controller contract, not a
Gateway-local choice**

The same path is used identically across both data-plane and control-plane
binaries:

- Gateway: `src/core/gateway/api/mod.rs:513` — gated by `is_integration_testing_mode()`
- Controller: `src/core/controller/api/mod.rs:441` — gated by `is_test_mode()`

Auth/authz wiring follows: `src/core/common/unified_auth/mod.rs:144` adds the
path to the auth skip set, and `src/core/controller/api/authz_classifier.rs:62`
classifies it under `Verb::Debug`. The `Verb::Debug` taxonomy is the canonical
expression of "this is a diagnostic endpoint regardless of mount condition" —
again decoupling function from gate.

Promoting just the Gateway entry into `create_admin_router` (proposal 1 in the
F-B01 issue file) would create Gateway-vs-Controller divergence: Controller
would still gate the same path on test mode, but Gateway would expose it in
production. That divergence is a regression — the `cache_residue` validator
calls **both** Controller and Gateway versions in the same run and expects
symmetric availability.

**3. Renaming the path is high-cost and zero-value**

Proposal 2 in the F-B01 issue file (rename to `/api/v1/testing/store-stats`)
would touch ~20+ files: `examples/test/scripts/conf-sync/run_conf_sync_test.sh`
and `run_conf_sync_advanced_test.sh` (~15 call sites combined),
`examples/code/validator/cache_residue.rs` (4 occurrences including mock-server
paths), `src/core/common/unified_auth/mod.rs` (auth skip list),
`src/core/controller/api/authz_classifier.rs` (classifier mapping + its unit
test at `:272`), Gateway `mod.rs:513`, Controller `mod.rs:441` +
`debug_handlers.rs:51` doc comment, and 4 skill docs that document the path as
canonical (`skills/05-testing/02-cache-residue-detection.md`,
`skills/05-testing/04-conf-sync-leak-testing.md`,
`skills/06-tracing/00-debugging.md`,
`skills/04-review/architecture/route-architecture-consistency.md:152`).

The blast radius is broad and demands Gateway+Controller coordination for a
cosmetic prefix change, with no functional benefit. The boundary model
documented in §1 above means the prefix carries no contract — readers learn the
gate from the router topology, not from the path name.

**4. The "operator can't reach it in production" framing is not a defect**

The F-B01 issue file frames the silent-404 in production as observability loss
for operators. This inverts the design intent: `store-stats` is not an operator
diagnostic — it is a test-fixture state dump. Its consumers are the leak
detection scripts and the `cache_residue` validator, both of which run with
`--integration-testing-mode` explicitly. The production operator's diagnostic
surface is `/health`, `/ready`, `/api/v1/server-info`, and the
`/configclient/{kind}` family — none of which are affected.

Exposing 20+ internal store counters to production operators would also
constitute a new disclosure surface that has never been threat-modelled (cache
sizes can leak resource counts for kinds the operator may not have RBAC on at
the K8s layer). The current gating is conservative and correct.

## Fix Suggestions Not Accepted

- Move `/api/v1/debug/store-stats` registration from `create_testing_router`
  into `create_admin_router` — breaks three-binary contract (Controller/Center
  still gated by test mode), creates a new production disclosure surface that
  was never threat-modelled.
- Rename `/api/v1/debug/store-stats` to `/api/v1/testing/store-stats` to match
  sibling-route prefixes — ~30-file blast radius across scripts/validator/auth
  /authz/docs/three binaries, zero functional change, inverts the
  function-vs-gate decoupling documented above.
- Add a third intermediate router (e.g. `create_debug_router`) to make the
  prefix-vs-gate mapping name-aligned — adds structural complexity for the
  cosmetic concern only, multiplies the three-binary coordination surface.

## Re-evaluation Triggers

Re-open this decision only if:

- A concrete production operator use case emerges for `store-stats` (cache
  residue debugging on a live cluster), at which point the proper response is a
  new, redacted `/api/v1/admin/store-summary` endpoint behind operator auth —
  not promoting the test-fixture version.
- The testing-mode gate model itself changes (e.g. controllable at runtime
  instead of process-startup-only), at which point the disclosure surface needs
  to be re-evaluated holistically — not just for `store-stats`.
- Gateway and Controller diverge for an unrelated reason such that the
  symmetry argument no longer holds.

## Reference Cases

- `tasks/infra/infra-cluster-FB01-api-store-stats-in-testing-router-not-debug.md`
  — original finding (Low, BadLogic), closed as ⚠️ accepted tradeoff with this
  rationale; also indexed at `tasks/gateway-deep-review/infra-cluster/INDEX.md`
  and recorded in `tasks/gateway-deep-review/confirmed.md:889` /
  `tasks/issues/_REVIEW-LOG.md:143`.
- `skills/04-review/architecture/testing-router-error-body-enumeration.md`
  — the parent boundary-model decision: testing router is one fixture surface,
  gated by `--integration-testing-mode` at process start.
- `skills/04-review/architecture/route-architecture-consistency.md:152` —
  documents `/api/v1/debug/store-stats` as the canonical leak-detection API
  with the current prefix.
- `skills/05-testing/04-conf-sync-leak-testing.md:84` and
  `skills/05-testing/02-cache-residue-detection.md:16-17` — confirm the
  endpoint's testing-mode-only contract from the consumer side.
- `src/core/gateway/api/mod.rs:513` (Gateway route),
  `src/core/controller/api/mod.rs:441` (Controller route),
  `src/core/controller/api/debug_handlers.rs:51` (Controller handler doc),
  `src/core/common/unified_auth/mod.rs:144` (auth skip),
  `src/core/controller/api/authz_classifier.rs:62` (Verb::Debug mapping).
