# B0157 — Stream Plugin Store

## Scope

- Analysis and implementation HEAD: `393267a1d67700a53a0b87903034538659e116dc`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/stream_plugin_store.rs`
- Baseline blob: `d88f535587ba4fab4b8350782dc67d4eda7924c1`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: the complete store, both runtime families, conf-sync publication, every caller, snapshot ownership, tests and relevant concurrency precedent were inspected. Decision: change; simple one-file correction.
- Implementation: renamed the internal delta mutator to `apply_delta`, narrowed `replace_all` to crate visibility, and documented that clone-and-store correctness relies on the serialized `CacheData` writer. Runtime-family documentation now distinguishes connection-context execution from route-scoped TLS execution.
- Preserved contracts: one `ArcSwap` publication per update, update-over-add and remove-last semantics, key identity, statistics, logging, immutable generation pinning and production conf-sync behavior are unchanged.
- Tests: three store tests passed. New coverage proves full replacement and clearing, old snapshot usability, partial add/replace/remove behavior, pointer-distinct replacement, and unchanged-entry `Arc` reuse.
- Review: independent review returned LGTM with no Must Fix, Should Fix or deferred finding. It verified that the production write lock spans `full_set` and `partial_update`, no workspace API consumer is broken, and no integration-test edit exists.
- Verification: `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc validation, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards passed; Clippy retained only existing warnings.
- External gate repair: a read-only diagnostic verified that external commit `393267a1` introduced four links to omitted task records. After explicit user authorization, a separate analyst confirmed that each link represented real residual work and froze an exact four-file allow-list. A separate implementer created the HMAC quick-start, ACME DNS error-redaction, credential-ownership follow-up, and OIDC response-cap tasks; independent review returned LGTM. The validator was not weakened and the source knowledge page was not changed.
- Integration coverage: no existing integration test was edited and no new integration-test gap was identified.
- Commit: pending local batch commit; never push.
