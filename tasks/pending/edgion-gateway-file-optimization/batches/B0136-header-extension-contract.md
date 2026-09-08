# B0136 — Header Extension Contract

## Scope

- Baseline and promotion HEAD: `2fffc7049cb58f8a3831e7a328d57453935b5485`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/traits/header_ext.rs`
- Baseline blob: `427615528ae815e5487bbf7b314e54f0bb41a4cf`
- Exact implementation allow-list: owned file only, including its inline tests
- Protected: every other source/test file, all existing `edgion-tests/**`, and unrelated user paths

## Handoff

- Analysis: `b0136_header_ext_analysis` read the complete extension trait, production adapter, all 22 direct consumers, inline tests, HTTP 1.5 HeaderMap contract/implementation and duplicate-header review knowledge. Decision: change; simple documentation/test-name correction.
- Value: `iter_headers` currently promises global order, but `HeaderMap::iter` guarantees arbitrary cross-name order; only same-name values retain insertion order. Correct `header_all_bytes` and `iter_headers` documentation to state exact per-name guarantees, and rename the existing test to identify what it actually proves. Preserve every runtime byte and behavior.
- Preserved contracts: allocation-free borrowed access, first-value UTF-8 behavior, case-insensitive names, same-name duplicate insertion order, live mutation visibility, and all auth/CORS/DSL/ExtProc/RealIp/RegionRoute/mirroring/outbound/WAF/Gateway API consumers.
- Deferred: the known multi-line Connection consumer gap remains owned by existing HTTP/1 review knowledge; do not expand this generic helper batch.
- Implementation: corrected the same-name versus cross-name ordering contracts and renamed the existing duplicate-header test to state its per-name scope. Runtime code and assertions are unchanged.
- Review: `b0135_final_review` independently approved B0136 with no Must Fix or Should Fix. It verified the locked HTTP 1.5 HeaderMap guarantees, every direct consumer's ordering needs, the no-wire-order warning, exact one-file scope, and unchanged runtime/assertions.
- Verification: focused header extension tests passed 3/3. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,090-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `2963756d`; local only, never pushed by the campaign.
