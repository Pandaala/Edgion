# B0286 — HTTP ExtProc Plugin

- Pipeline state: analysis-ready; change candidate, complex / plan-first.
- Active anchor: `edgion-gateway`; baseline `6bf48dcf`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`.
- Analyst: `b0286_extproc_plugin_analysis`; full plugin-owned lifecycle/value and coverage analysis assigned. Prior B0282 supporting edits did not establish full coverage.
- B0284 owns the serial commit lane; B0285 query correction is queued ahead and may adapt one test expectation here. Refresh this candidate against intervening commits before promotion.
- No edits authorized; existing integrations and user handoff/WAF paths protected. Existing D0148-D0151 and queued query/opaque-header findings must not be duplicated.
- Full plugin and consumers reviewed; ImmediateResponse protobuf bytes pass through lossy String conversion, corrupting HTTP binary replies and copying valid text. Header and streamed request paths share the issue; a header-only partial fix is insufficient.
- Proposed support scope: plugin.rs, streamed.rs, runtime/traits/request_body_stream.rs, and only write_terminate_reply plus directly related tests in pg_request_body_filter_action.rs; no WAF branch edits. Correct unification section 13's stale String exception and extend existing local-response-body-is-clone-cheap-bytes knowledge.
- Plan must explicitly settle canonical gRPC valid/invalid text and exact status behavior while preserving HTTP Bytes end to end, existing caps/suppression/failure contracts. Deterministic helper/mock/ScriptedExchange/writer tests only; no socket/server or integration edits.
- At 989d2a27 target blob `9390e125a91282119939fba39bd8caca59222555`; supporting streamed `d4a2a2bfaebf8897422c4561281d6679fe0e8c28`, stream trait `42975c30e280f82621fec5e1c9bc2ededa02c381`, body action `4898e0022ecd8148b70acff0c883f647b2c61002`. Refresh after B0285 before promotion.
