---
name: region-route-effective-table-validation
description: Use when reviewing RegionRoute table validation, overlay fail-to-base, consumer modulo checks, or failover edits.
knowledge_type: regression-guards
---

# RegionRoute validates complete tables before request selection

## Conclusion and scope

`RegionRouteConfig.regions` is the safe base table for an EdgionPlugins routing
entry. `RegionRouteOverride.regions` in EdgionConfigData optionally replaces that
table whole; `None` inherits and `Some([])` is invalid. A non-empty `route_rules`
requires a non-empty base table even with `override_ref`; no-rule configurations
may have an empty base and remain no-ops. Non-empty tables are always validated.

The shared resources-layer `RegionTableValidation::new` validates nonblank unique
names, endpoint syntax, ordered disjoint closed intervals, existing nonself
failover targets, and an acyclic failover graph. An interval ending at `65535`
cannot fit any supported u16 modulo and is invalid even without a consumer.
`validate_modulo` / `accepts_modulo` check the compact maximum-bound proof against
each consumer's domain. The effective modulo is `1000` without `hash_calc` and
otherwise must be at least `2`. Gaps remain valid and preserve pass-through.

Endpoints accept hostnames, IPv4, and bracketed IPv6 with optional ports in
`1..=65535`; URLs, empty hosts, and malformed ports are invalid. Hostname spelling
and trailing dots are preserved, IPv6 brackets are removed for the host value,
and absent ports use the existing TLS-based defaults. Syntax validation performs
no DNS or address-policy work; canonical upstream address safety remains
required after DNS resolution.

Failover execution remains one hop. Acyclic `a -> b -> c` is valid: selecting `a`
redirects to `b`, not `c`. Every region, including failover sources, owns a
disjoint hash interval; failover is not a reason to exempt its interval.

## Rationale and evidence

Source `D0065` identified inconsistent base, overlay, and effective validation;
`B0099` had already handled the separate modulo-zero panic. The user approved the
complete decision matrix and cross-component implementation on 2026-09-08.
The base-plus-overlay authority is
`skills/01-architecture/05-resources/15-edgion-config-data.md`; request-path cost,
shared validation, and atomic publication follow `skills/00-design-rules.md`.

Decisive owners and call chains:

- `edgion-resources/src/resources/edgion_config_data/region_route.rs` owns the
  table validator and proof; plugin configuration validation and overlay
  standalone validation delegate to it.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/region_route.rs`
  owns `RegionDef::parse_endpoint`, base validation, and consumer modulo checks.
- Controller EdgionPlugins handling keeps invalid-base `Accepted` separate from
  invalid-overlay `ResolvedRefs`. Generic ingest remains advisory; those status
  conditions alone do not establish Gateway enforcement.
- `edgion-controller/src/api/region_route_handlers.rs` excludes invalid bases
  from the effective view and checks an overlay for each consumer. Dedicated
  failover edits validate the candidate table before persistence; table errors
  return `422` without writing.
- Gateway `EdgionConfigDataStore` publishes the raw entry and compact table proof
  in one snapshot. `RegionRoute::create` validates base configuration;
  `compute_effective_regions` borrows the raw overlay and checks its proof's
  modulo in constant work. Full-table graph/endpoint/range checks do not run per
  request. The selected endpoint is still parsed by the shared strict parser
  before constructing the existing external jump.

## Alternatives and limits

Missing, disabled, wrong-type, or invalid overlays fall back to the valid base,
without retaining a last-good overlay or adding TTL semantics. A table can be
standalone-valid yet invalid for a consumer with a smaller modulo. Generic
resource admission is not converted into hard rejection; the dedicated failover
mutation gate is deliberately narrower.

The
[atomic publication rule](../../plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md)
previously stated that overlays needed no derived work. That claim is superseded
by the useful compact proof, not by reintroducing a duplicate region table.
The
[retained management API decision](../../admin-api/intentional-design/controller-region-route-endpoints-retained-for-drill-in.md)
still requires covering effective/failover endpoints. Its historical external
Center caller evidence is not a fresh audit of that separate repository.

## Verification and re-evaluation

Original implementation verification on 2026-09-08 (historical; the unit tests
below were not rerun by the subsequent integration follow-up):

- `cargo test -p edgion-resources --lib region_route`: 16 passed.
- `cargo test -p edgion-gateway -p edgion-controller --lib region_route`:
  25 Gateway and 25 Controller tests passed.
- `cargo test -p edgion-gateway --lib plugins::runtime::edgion_config_data`:
  18 passed, including raw/proof replacement, invalidation, disabled/variant/
  omitted-table transitions, recovery, removal, and held-generation consistency.
- `cargo test -p edgion-controller --lib failover`: 15 passed (some overlap
  with the Controller RegionRoute filter).
- `cargo fmt --all` followed by `cargo fmt --all -- --check`,
  `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed. Clippy exits successfully
  with existing workspace warnings; this is not a warning-free baseline.
- Agent-doc, SSA-force, metrics-inventory, Pingora-source-policy and Gateway
  no-tracing guards, plus `git diff --check`: passed.
- A fresh independent read-only reviewer returned LGTM after corrections to
  the concurrent test-store operation and selected-endpoint parsing description.

The original correction did not run integration suites. The D0065 integration
follow-up on 2026-09-08 subsequently migrated `region-route-myregion` to disjoint
intervals without changing its direct-region or one-hop assertions, and added
`RegionRouteValidation` in `edgion-tests/src/client/dynamic/scenarios/region_route_validation.rs`.
The [scenario contract](../../../../edgion-tests/integration/conf/EdgionPlugins/RegionRoute/DynamicTest/README.md)
documents fixture dependencies, the transition matrix, and synchronization limits.

Fresh follow-up verification:

- `cargo build -p edgion-tests --bin test_client -p edgion-gateway --bin edgion-gateway
  -p edgion-controller --bin edgion-controller -p edgion-cli --bin edgion-cli`: passed;
  the revised client was rebuilt with `cargo build -p edgion-tests --bin test_client`.
- `target/debug/test_client --list-dynamic-scenarios`: passed, including the new
  scenario's inventory and listener contract.
- `target/debug/test_client -g -r EdgionPlugins -i RegionRoute`: 19 passed.
- `target/debug/test_client -g --dynamic-scenario RegionRouteValidation`: passed
  against a shared local FS Controller/Gateway/test_server topology with conf_sync
  mTLS. Only `rrv-*` resources mutate within the scenario; cleanup also passed.
- Latest `cargo check --workspace --all-targets` and
  `cargo clippy --workspace --all-targets`: passed with existing Clippy warnings.
- `cargo fmt --all` and the subsequent format check, agent-doc consistency,
  SSA-force, metrics-inventory, Pingora-source-policy, Gateway no-tracing guards,
  and `git diff --check`: passed.
- A fresh independent read-only reviewer returned LGTM for the final tests and
  knowledge update, including the observed HTTP envelopes and access-log shape.

The new scenario checks Controller status, Gateway raw publication, each consumer's
effective HTTP view, and fresh traffic independently. A valid replacement selecting
`b` precedes each invalid/inert/missing transition, which must fall back to the
local `base`, then recover to `a` without reapplying EdgionPlugins. Modulo 100 and
1000 consumers sharing one overlay prove that standalone validity is insufficient.
Management HTTP checks verify 422 and unchanged stored content for invalid
failover candidates, and successful writes form `a -> b -> c` while requests stop
at `b`. An invalid base is excluded from the effective view.

Remote selection is proved by the exact RegionRoute access-log target plus
`BackendLocalhostNotAllowed`; loopback rejection remains active. This does not
claim successful remote network delivery or DNS-rebinding coverage. Local base
traffic must reach the shared test backend. Polling establishes eventual agreement
after serial updates, not a cross-API atomic snapshot. Kubernetes execution and
the full workspace test suite were not run for this follow-up.

Re-evaluate when hash domains widen, routing rules gain new table dependencies,
failover becomes recursive, endpoint syntax changes, or overlay publication
gains a second writer. Revalidate both raw/proof generation consistency and
per-consumer bounds when changing store reuse projections.

## Resolution references

- `D0065` — fixed configuration/runtime validation, 2026-09-08; the subsequent
  `region-route-validation-integration` follow-up verified fixture migration,
  hot reload, per-consumer effective tables, and management HTTP behavior.
- `B0099` — earlier, separate modulo-zero guard; not evidence of complete table validation.

### Override identity boundary

RegionRoute override lookup keys now come from an actual EP entry loan. Invalid owner identity prevents reference-bearing construction; an invalid, denied or unavailable target leaves the validated base table in force. Cold cache eligibility checks this distinction before reuse. Readonly Controller diagnostics validate current identity and permission for enabled RegionRoute entries. This does not change base-table validation, effective-table proof publication, or the Gateway paired raw/proof snapshot lookup.
