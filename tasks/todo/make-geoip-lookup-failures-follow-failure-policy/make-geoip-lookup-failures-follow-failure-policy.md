# Make GeoIP Lookup Failures Follow Failure Policy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Distinguish ordinary GeoIP record absence from MMDB decode failures and caught reader panics, then route true lookup failures through each HTTP and stream consumer's existing `failOpen` contract. The design must preserve bounded request-path observability and access-log meaning without turning malformed lower-trust database data into an allow-by-default `NotFound` result.

## Requirements

- Within `GeoIpProvider::lookup`, reserve `NotFound` for a successful MMDB lookup with no record for the normalized IP. Preserve the request-context mapping of an unparseable client address to `NotFound` unless a separate design explicitly changes it.
- Represent a loaded-database decode error or caught reader panic as an explicit lookup failure, either with a new typed outcome or a carefully reviewed widening of the unavailable outcome. Do not collapse it into missingness.
- Make HTTP rule mode and the existing Stage-1 stream GeoIP consumer apply their current `failOpen` behavior to lookup failures. HTTP parse-only mode remains pass-through while exposing the distinct bounded access-log status. Do not add another failure-policy shape or change a configured default.
- Preserve `Found` projection, IPv4-mapped normalization, direct-peer versus remote-IP ownership, allow/deny plus `defaultAction`, and layer-specific stream-denial metrics.
- Define the access-log representation for lookup failure so it is distinguishable from ordinary `NotFound` and database-unavailable state without exposing an IP, MMDB record, decoder detail, address, trace, or database path.
- Reconcile the existing panic counter and metrics inventory with the typed failure outcome. Avoid per-request tracing and high-cardinality labels; use a bounded reason vocabulary only if an additional metric signal is necessary.
- Keep the provider lock-free on the request path and preserve atomic database publication.
- Add a deterministic synthetic malformed-record or decoder seam that proves decode failures and caught panics reach failure policy. Do not depend on corrupting the shared integration fixture.
- Preserve existing integration tests. Add a separate integration-test task only if design/implementation proves the typed failure path cannot be exercised adequately by provider and consumer unit tests.

## Scope

### In scope

- `edgion-gateway/src/geoip/info.rs`
- `edgion-gateway/src/geoip/provider.rs`
- `edgion-gateway/src/plugins/http/geoip_location/plugin.rs`
- `edgion-gateway/src/plugins/stream/geoip_location/plugin.rs`
- `edgion-gateway/src/observe/access_log/entry.rs`
- GeoIP metrics registry, inventory, and canonical operator/developer documentation only as required by the selected bounded outcome model.
- Source-local provider, HTTP, stream, access-log, and metrics tests.

### Out of scope

- GeoIP updater download, validator, TLS, egress, or hot-swap behavior.
- Changing the GeoIP rule schema, `failOpen` defaults, client-IP trust boundaries, or policy matching semantics.
- Per-IP, decoder-detail, database-path, or request-derived metric labels and tracing.
- New TCP/TLS GeoIP integration or changes to request-address parsing semantics.
- Modifying existing `edgion-tests/**` files.

## Acceptance criteria

- Successful no-record lookup remains `NotFound`; unavailable database remains separately identifiable; decode error and panic are never reported as ordinary missingness.
- HTTP rule mode and the existing Stage-1 stream consumer demonstrably fail close on lookup failure when `failOpen` is false and continue according to the existing open policy when true; HTTP parse-only remains pass-through and records the bounded failure status.
- Access logs distinguish lookup failure with a fixed bounded status and no sensitive/internal detail.
- Existing panic observability is retained or deliberately replaced without double counting, per-request log amplification, or metric-label growth outside the inventory allow-list.
- Synthetic malformed/decode and panic regressions cover the provider-to-consumer path; existing Found/NotFound/Unavailable, IPv4-mapped, and policy tests remain green.
- No existing integration test changes. A future integration issue is created only if the reviewed implementation cannot establish the failure-policy path with deterministic source-local tests.
- Formatting, workspace check, Clippy, documentation validation, metrics inventory, Gateway no-tracing, and relevant repository guards pass.

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| `make-geoip-lookup-failures-follow-failure-policy.md` | done | Analysis boundary and acceptance contract |
| `00-context.md` | not created | Create before implementation |
| `01-design.md` | not created | Required before implementation |
| `02-implementation.md` | not created | Create after independent design approval |
| `03-subtasks.md` | not created | Create with the approved cross-module plan |

## Affected modules

| Module | Impact |
|--------|--------|
| GeoIP provider and outcome model | Preserve error identity across the MMDB boundary |
| HTTP and stream policy consumers | Apply the existing failure policy consistently |
| Access log and metrics | Report a bounded failure class without leaking detail |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer from B0059 as a design-first cross-consumer bugfix. | A provider-local enum edit would leave consumer failure policy, access-log meaning, and observability inconsistent. |
| 2026-08-31 | Do not create an integration-test issue yet. | A deterministic malformed-record or decoder seam plus provider and consumer unit tests should cover the failure path; add integration work only if implementation proves otherwise. |
