---
name: region-route-effective-table-validation
description: RegionRoute required setting validation, atomic identity/table publication, unavailable-setting policies and failover edits.
knowledge_type: regression-guards
---

# RegionRoute validates complete tables before request selection

## Current contract (2026-10-30 migration)

ClusterRegionSetting replaces the optional regional override. Its required
clusterName/regionName/regions define identity and the complete table. regionName
must name a row. RegionRoute requires clusterRegionRef and owns no base table.
The shared RegionTableValidation still checks unique nonblank names, strict
endpoint syntax, disjoint closed ranges and an acyclic failover graph. Each
consumer checks the proof against its modulo; gaps and one-hop failover remain.

Raw setting, local identity and proof publish atomically. Requests borrow one
resource generation without cloning or validating the full table. Missing,
disabled, denied, wrong-type or invalid settings execute failOpen (default true)
or statusOnError (default 503) before any request mutation. Operational errors
must not cause compilation to discard a Reject owner. Structural/policy errors
remain generic rejection; owner identity, conditions, body and dye safety gates
are preserved. Dependency and grant changes use existing requeue/bindings.

The effective API returns explicit Controller observation fields, including
configuration availability. It must not expose arbitrary processed config or
claim Gateway application. Dedicated mutation validates the full setting before
writing. Source paging and the dedicated CenterWatch directory are implemented. Center
reads lightweight identities and declared references; details and preview/CAS
remain fresh Controller operations. See the CenterWatchStore architecture guide.

Current implementation and checks are tracked in the source-browsing task; the
historical test counts below are not fresh verification. Prior references below
to base tables, optional overlays and their fallback are superseded by this section.

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

## Canonical failover mutation table (ARCH-146)

The dedicated Controller command mutates only the matching `RegionRouteOverride`
entry at `spec.data`, the same table checked by `validate_failover_resource` and
consumed by effective routing. An obsolete `spec.plugins` array in a raw filesystem
resource is preserved as unrelated input; it cannot select the mutation target or
produce an already-applied success while the canonical table is unchanged.
Missing/wrong-type canonical entries and missing canonical regions retain the
existing not-found outcome. Complete-table validation still applies before writes
and before an idempotent success. Locking, conditional writes, retries and version
allocation are unchanged.

The in-process filesystem regressions in `failover_version_tests.rs` exercise a
mixed old/new document through the real helper: set, repeat, clear and repeat;
they compare the complete stored spec and version behavior. Rejected invalid-target
and idempotent-invalid-table commands return 422; missing/wrong-type/table-region
cases return 404. All rejected cases preserve stored bytes, including the version.
These tests cover the storage command boundary, not new Gateway traffic behavior.


## Managed overlay integration evidence (ARCH-355)

The two managed-overlay plugin cases verify the actual enabled RegionRoute
`overrideRef` identity as well as Controller-local Accepted=True and
ResolvedRefs=True. When the resource supplies a generation, both observations
must match it; generation-less native filesystem/etcd status remains supported.
Missing, malformed, stale or unresolved observations keep the existing bounded
poll unsatisfied. Structural acceptance alone permits safe base fallback and
cannot prove the overlay edge.

Pure predicate tests cover native and partitioned status, unresolved or
mistargeted references, disabled entries, stale/missing conditions, and malformed
identity/generation inputs. Existing ConfigData presence/type checks and polling
IO behavior remain unchanged. These checks do not establish a live managed-overlay
or data-plane acceptance run.

## Independent failover selection and rejection evidence (ARCH-390)

The `my_region_failover_to_nonlocal` integration case scopes its trace-correlated
access log to route `region-route-myregion-test` and exactly one RegionRoute
execution in plugin resource `region-route-myregion`. Its private observation
predicate requires separate canonical message-array entries for exact
`failover europe→ohio` and target `→ohio 127.0.0.1:30002` with an endpoint delimiter,
plus the typed `error` diagnostic `BackendLocalhostNotAllowed`. The redirect alone
cannot prove endpoint selection, and the target message precedes the jump setter,
so it cannot substitute for terminal loopback rejection. HTTP 500/502 remains a
separate response check.

Focused predicate regressions cover wrong ports (including a prefix collision),
redirect-only and target-only observations, missing or unrelated diagnostics,
wrong route/resource/plugin, missing or duplicate execution, malformed log shapes
and matching text outside the selected execution. These synthetic observations
test assertion discrimination; they do not establish live integration success or
successful remote delivery. The original fixture, request/retry contract,
one-hop behavior and loopback guard remain unchanged. Re-evaluate the assertions
if canonical plugin message serialization, fixture identities/endpoints or the
terminal diagnostic contract changes.

## Borrowed selection inputs

RegionRoute uses the canonical session adapter `key_value` within its own
first-nonempty source loop. Header, query, cookie, context and other canonical
session strings remain borrowed; RemoteIpAndPath retains owned composition.
Source references belong to configuration independently of the session borrow.
Exact bypass membership and map lookup consume string slices, and hashing and
sensitive logging finish before session writes or route mutation.

This changes input ownership only: rule/source order, empty-value skipping,
configured map overrides, hash passthrough and writeback, effective overlay
validation and one-hop failover remain unchanged. Focused tests cover source
identity and borrowed/owned shape plus direct/map/bypass and hash request paths.
No throughput or callback-wide zero-allocation claim follows.

## Fixed writeback diagnostic targets

RegionRoute prepares the joined writeback target names once in its shared bound/
inline constructor. The private String follows the immutable base configuration;
overlays replace only regions. Preparation uses canonical KeySet::as_log_str and
preserves order, duplicates and commas. Empty/absent writeback lists allocate no
label payload. Construction shrinks the joined String to avoid retaining its growth
slack. This adds one String owner plus its capacity per plugin instance,
including configurations that never take the computed-hash branch.

Request execution still attempts every setter with the current hash, ignores the
same setter failures and logs the targets after all writes, before range lookup.
Precomputed-hash, missing-input and empty-list paths retain their previous skips.
Targets describe attempted writes, not successful ones. The raw prepared text
passes through sensitive at request logging time; construction-time masking never
becomes part of the prepared data. No new log, metric, interface or queue exists.

Regression coverage compares all four target kinds and duplicates, bound/inline
construction, old/new configurations, failed writes, computed no-range and hash
passthrough. Sensitive Off/Replace/Wrap modes run in the existing isolated test
subprocess mechanism, with construction and request modes varied independently.
Allocation evidence separates removed repeated target formatting from the retained
String capacity and the unchanged hash-value, setter and log work. It makes no
whole-request allocation-free or throughput claim.
