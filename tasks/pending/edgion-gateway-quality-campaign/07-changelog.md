# Campaign Changelog

| Batch | Date | Files | Outcome | Actual improvement | Verification | Commit | Follow-ups |
|-------|------|-------|---------|--------------------|--------------|--------|------------|
| B0001 | 2026-09-19 | `edgion-gateway/Cargo.toml` and approved manifest/lock/docs support | changed | Removed unused Gateway and SQLite dependency surface, redundant test declarations, and a dead feature; corrected active FIPS/Pingora guidance | Full gates passed after independent rustls remediation; frozen tests unchanged | `28de28f50` source, `8a32f09f0` records | D0001 allocator-feature consistency |
| B0002 | 2026-09-19 | `edgion-gateway/build.rs` | changed | Invalid multiple-allocator selections now fail early with one stable Gateway diagnostic instead of secondary type errors | Focused feature matrix and full repository gates passed; frozen tests unchanged | `48643849b` | none |
| B0003 | 2026-09-19 | `edgion-gateway/src/api/admin_auth.rs`; `api/mod.rs` examined | changed + examined-no-change | Added RFC 6750-compliant static Bearer realm and middleware tests; confirmed merged-router auth already covers all Admin/testing routes | 9 targeted tests and full repository gates passed; frozen tests unchanged | pending automatic commit | D0003, D0004 |
