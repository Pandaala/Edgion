# B0053 — HTTPRoute Conf Handler Guidance

## Scope

- Baseline: `c537bb09fc4cc0064998eae2f93980f4d856a073`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned code file: `edgion-gateway/src/routes/http/conf_handler_impl.rs`.
- Allowed knowledge files:
  - `skills/01-architecture/05-resources/04-http-route.md`
  - `skills/04-review/conf-sync/intentional-design/conf-handler-simple-map-pattern.md`
  - `skills/04-review/concurrency/review-evidence/lb-false-positives.md`
  - `skills/04-review/routing/intentional-design/route-architecture-consistency.md`
- Existing `edgion-tests/**` files and every unrelated source, knowledge, and protected user path remain forbidden.
- Exact commit allow-list: the five files above plus this batch record, campaign ledger, main file, and changelog.

## Context and value gate

- Analysis verdict: `change`; implementation complexity: `simple`.
- Runtime publication, locking, incremental rebuilds, stale-listener cleanup, fail-closed validation, immutable snapshots, and request matching remain sound.
- The target test commentary and four maintained knowledge pages still describe a removed HTTPRoute-owned LB-policy store or its deleted synchronization helpers.
- Canonical backend traffic and load-balancing policy ownership now belongs to typed `EdgionBackendTrafficPolicy` through `BackendTrafficPolicyStore`; correcting the stale guidance prevents a parallel mechanism from being reintroduced.

## Implementation brief

1. Correct the target test-block ordering commentary and rename its ExtensionRef fixture helper and parameters so they no longer imply a live route-owned LB policy.
2. Replace obsolete HTTPRoute LB-policy synchronization guidance with the current plugin-admission and adaptive-contribution publication responsibilities.
3. Correct comparison and precedent pages to describe the current Arc cache/incremental pipeline and canonical EBTP ownership; retain superseded history only when clearly marked historical.
4. Preserve all runtime behavior, schema, protocol, publication, observability, and test semantics.

## Verification plan

- Focused route conf-handler generation/cache tests.
- Agent documentation validation.
- Full campaign repository gates and frozen-path verification.

## Implementation

- Corrected the ConfHandler cache test commentary so it describes only cache replacement, Arc sharing, and unchanged-replay generation identity, without claiming to test inter-step ordering.
- Renamed the local `build_route_with_lb_policy` fixture to `build_route_with_extension_ref` and renamed its parameters to match its actual ExtensionRef role.
- Replaced deleted route-owned LB-policy synchronization guidance with current plugin-admission and adaptive-contribution publication responsibilities.
- Identified typed `EdgionBackendTrafficPolicy` through `BackendTrafficPolicyStore` as the canonical traffic/LB policy owner, while retaining obsolete `PolicyStore` material only as explicitly superseded history.
- No runtime, schema, protocol, publication, observability, metric, or test behavior changed.

## Independent review

- The first review found two overstatements about what the existing tests prove.
- The implementer narrowed both claims to verified cache and rebuilt-route post-conditions and separated actual implementation order from regression-test coverage.
- Re-review passed with no remaining Must Fix, Should Fix, or deferred finding.

## Verification

- The focused HTTPRoute ConfHandler suite passed all 42 tests; the two directly targeted cache/generation tests also passed independently.
- Formatting, workspace check, workspace clippy, agent-doc validation, SSA, metrics inventory, Pingora source policy, Gateway no-tracing, cargo-deny, English-only, and diff checks passed.
- Only the campaign's known Kafka and frozen integration-test clippy warnings plus cargo-deny duplicate/advisory-ignore warnings remain.
- Frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `7edf2384b59f80736182717394f24561d8bd683c`.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
