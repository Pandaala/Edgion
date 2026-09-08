# B0140 — Plugin Session Trait

## Scope

- Baseline: `230c934675029ce69fefdaa7eabae08c6420fd1a`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/traits/session.rs`
- Baseline blob: `a3338ee2c31f65bee36b5ecb13c6bdcc35e03fe5`
- Proposed implementation allow-list: owned file only

## Handoff

- Analysis: `b0140_session_trait_analysis` read the complete 943-line trait, sole production adapter, generated mock, every method consumer family, body gates/processors, URI/header/TLS/IP callers and relevant tests/knowledge. Decision: change; simple documentation-only correction.
- Value: body-operation documentation still says request buffering is unavailable, promises a future timeout runtime, and describes full upstream response buffering. Current production implements bounded request capture, distinguishes downstream timeout from storage expiry, and permanently streams the main upstream response. Correct the docs to the executable and mandatory architecture contract.
- Proposed scope: redefine `Unimplemented` as unavailable for the selected adapter/configuration; correct Timeout and StorageFailure distinction; document request-only shared capture and permanently streaming upstream responses; describe `read_response_body` as a defensive compatibility seam that never aggregates the main response and may return empty only for verified bodyless responses.
- Preserved contracts: all signatures/error mappings/defaults/object safety, one BodyStorage, streaming/backpressure, URI/header/TLS/IP semantics, hot-path performance, failure policy and observability.
- Deferred: resources-layer generic body-requirement documentation and stale D0035 reconciliation remain separate ownership.
- Freshness: promoted at `99a94a73`; owned blob remains exactly `a3338ee2c31f65bee36b5ecb13c6bdcc35e03fe5`.
- Implementation: corrected only the body-operation documentation to distinguish bounded shared request capture, selected-operation unavailability, downstream capture timeout, storage failure, permanently streaming main responses and the fail-loud defensive response-body seam. API and runtime bytes are unchanged.
- Review: `b0135_final_review` independently approved with no Must Fix or Should Fix. It traced the production adapter's request storage, timeout/error mappings, unanimous bodyless proof, response streaming and existing adapter tests, and confirmed all URI/header/TLS/IP/mock/failure/log/observability behavior is unchanged.
- Verification: exact one-file documentation diff; `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,090-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: pending local batch commit; never push.
