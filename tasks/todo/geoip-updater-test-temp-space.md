# Remove the GeoIP Updater Test's Host-Temp-Space Assumption

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | testing |
| Priority | P3 |
| Issue | N/A |
| Depends on | B0093 |

## AI guide summary

Make the successful GeoIP update-cycle unit test independent of the host default temporary volume while preserving production disk-space safety behavior.

## Requirements

- Keep the production `ensure_free_space` calculation and failure policy unchanged.
- Size the success fixture's configured database cap to its bounded fixture data, or give the test an explicitly controlled temporary volume without relying on host-global free space.
- Preserve coverage of download, checksum validation, atomic rename, runtime database swap, and metadata persistence.
- Keep a separate deterministic test for insufficient disk space using an injected or isolated free-space seam if one exists; do not weaken the production guard.
- Do not modify existing integration tests.

## Acceptance criteria

- The success test passes on a constrained default temporary volume substantially smaller than 1 GiB.
- Disk-exhaustion behavior remains covered without depending on the executing host's real free space.
- GeoIP updater tests pass repeatedly both alone and within the full Gateway library suite.
- Workspace format/check/Clippy and repository guards pass.
