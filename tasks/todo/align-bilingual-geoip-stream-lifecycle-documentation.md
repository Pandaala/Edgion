# Align Bilingual GeoIP Stream Lifecycle Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | documentation |
| Priority | P2 |

## Requirements

- Update the English and Chinese GeoIP operations guides atomically.
- Replace Stage1-only L4 claims with the actual listener Stage1, TCP/UDP route
  and TLS compatibility lifecycle boundaries.
- Document the emitted bounded metric `layer` values without changing the
  already accurate general metrics inventory.
- State that listener Stage1 denial is metric-only, while TCP/UDP/TLS denial
  retains canonical protocol logging without GeoIP facts or sensitive reasons.
- Revalidate all claims against current call sites and frozen integration fixtures.
