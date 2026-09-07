# D0061 — Remove the GeoIP Updater Test's Host-Temp-Space Assumption

## Meta
- Status: resolved
- Severity: low
- Discovered in: B0093, full Gateway library verification
- Category: testing-environment
- Closed: 2026-09-05
- Canonical record: [`skills/04-review/observability/regression-guards/geoip-updater-success-test-fixture-scale-cap.md`](../../../../skills/04-review/observability/regression-guards/geoip-updater-success-test-fixture-scale-cap.md)

## Evidence
`successful_cycle_renames_swaps_and_persists_meta` created its fixture in the host default temporary directory while configuring a 1 GiB database cap. The production free-space guard therefore required at least 1 GiB before the fixture performed any HTTP work. The test deterministically failed with `UpdateError::Disk` on a 575 MiB temporary volume and passed repeatedly on a larger volume.

## Resolution
The success test now uses a fixture-scale `max_db_bytes` (`1 << 20`), matching the other updater cycle tests. Production `ensure_free_space` and its failure policy are unchanged. Disk exhaustion remains on `insufficient_free_space_fails_with_disk_reason_not_fetch` via an oversized injected cap. Weakening the production guard, skipping it under `cfg!(test)`, and changing existing integration tests were rejected.

## Integration-test impact
No integration issue is needed. This is a colocated deterministic unit-test setup problem; existing integration tests remain unchanged.
