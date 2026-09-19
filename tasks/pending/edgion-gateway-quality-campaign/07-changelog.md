# Campaign Changelog

| Batch | Date | Files | Outcome | Actual improvement | Verification | Commit | Follow-ups |
|-------|------|-------|---------|--------------------|--------------|--------|------------|
| B0001 | 2026-09-19 | `edgion-gateway/Cargo.toml` and approved manifest/lock/docs support | changed | Removed unused Gateway and SQLite dependency surface, redundant test declarations, and a dead feature; corrected active FIPS/Pingora guidance | Full gates passed after independent rustls remediation; frozen tests unchanged | `28de28f50` (external absorbing commit) | D0001 allocator-feature consistency |
