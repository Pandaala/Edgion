---
name: geoip-updater-success-test-fixture-scale-cap
description: Use when reviewing findings that the GeoIP updater success test requires a 1 GiB host temp volume or that ensure_free_space should be relaxed for tests; covers the fixture-scale max_db_bytes contract.
status: fixed
finding_id: D0061
closed: 2026-09-05
knowledge_type: regression-guards
---

# GeoIP updater success test uses a fixture-scale cap — FIXED

## Conclusion

`successful_cycle_renames_swaps_and_persists_meta` configures `max_db_bytes` at
fixture scale (`1 << 20`), matching the other cycle tests. First-start
`ensure_free_space` therefore only needs 1 MiB on the host default temporary
volume. The production free-space formula and failure policy are unchanged.
Disk exhaustion stays on a separate test that injects an oversized cap.

Fix suggestions of the form "skip `ensure_free_space` under `cfg!(test)`" or
"lower the production first-start requirement so the 1 GiB success-test cap
passes" are **not accepted**.

## Core Rationale

**1. The 1 GiB cap was a test configuration, not a production contract.**

The success fixture is a 2048-byte City MMDB. First start needs
`max_db_bytes` of free space. A `1 << 30` test cap therefore required 1 GiB
on the default tempfile volume before any HTTP work, and failed on a
~575 MiB tmpfs while the same test passed on a larger volume.

**2. Production disk safety stays load-bearing.**

`ensure_free_space` still requires `max_db_bytes + 2 * current_database_size`,
or `max_db_bytes` when no database exists. Weakening that guard to accommodate
a unit-test host would hide a real storage-sizing failure from operators.

**3. Disk exhaustion already has an isolated seam.**

`insufficient_free_space_fails_with_disk_reason_not_fetch` sets
`max_db_bytes = u64::MAX / 2` so the precheck fails before any request.
That path must stay deterministic and must not depend on the host's real
free space.

## Fix Suggestions Not Accepted

- Skip or stub `ensure_free_space` under `cfg!(test)` — hides the production
  guard from the success path.
- Lower the production first-start requirement — changes a safety boundary
  to satisfy a test host.
- Point the success test at a custom volume that still uses a 1 GiB cap —
  unnecessary once the cap matches the fixture.
- Change existing GeoIP integration tests.

## Re-evaluation Triggers

- The success fixture grows past the configured test cap.
- A new updater test reintroduces a GiB-scale `max_db_bytes` while still
  writing into `tempfile::tempdir()`.
- `ensure_free_space` gains an injectable free-space seam that a test starts
  using instead of a fixture-scale cap.

## Reference Cases

- Updater cycle tests: `edgion-gateway/src/geoip/updater.rs`
- Campaign finding: `tasks/pending/edgion-gateway-file-optimization/deferred/D0061-geoip-updater-test-temp-space.md`

## Resolution references

- `D0061` — 2026-09-05.
