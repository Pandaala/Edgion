# D0016 — AllowedSan regex ignoreCase contract

- Status: needs-investigation
- Canonical task: `tasks/todo/allowed-san-regex-ignore-case-contract.md`
- Discovered in: B0078, `edgion-common/src/allowed_san.rs`
- Analyst: `common_coverage`; independent reviewer `b0078_review` confirmed the classification.

The accepted regex flag is unused by canonical compilation. Contract resolution spans schema, compiler, docs and tests; the Common handshake matcher is unchanged.
