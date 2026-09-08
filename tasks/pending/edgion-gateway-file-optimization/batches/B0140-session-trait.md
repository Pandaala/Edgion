# B0140 — Plugin Session Trait

## Scope

- Baseline: `230c934675029ce69fefdaa7eabae08c6420fd1a`
- Pipeline state: analysis-ready
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
- Review: pending promotion, implementation and fresh final review.
- Verification: pending.
- Commit: pending local batch commit; never push.
