# D0061 — Remove the GeoIP Updater Test's Host-Temp-Space Assumption

## Meta
- Status: needs-implementation
- Severity: low
- Discovered in: B0093, full Gateway library verification
- Category: testing-environment
- Canonical task: [`tasks/todo/geoip-updater-test-temp-space.md`](../../../todo/geoip-updater-test-temp-space.md)

## Evidence
`successful_cycle_renames_swaps_and_persists_meta` creates its fixture in the host default temporary directory while configuring a 1 GiB database cap. The production free-space guard therefore requires at least 1 GiB before the fixture performs any HTTP work. The test deterministically fails with `UpdateError::Disk` on a 575 MiB temporary volume and passes repeatedly on a larger volume.

## Why deferred
The production free-space calculation is a safety boundary and must not be weakened to accommodate a unit-test host. The test should instead use a fixture-scale cap or an explicitly controlled temporary volume while retaining exact coverage of the guard, atomic rename, runtime swap, and metadata persistence.

## Integration-test impact
No integration issue is needed. This is a colocated deterministic unit-test setup problem; existing integration tests remain unchanged.
