# B0104 — ResponseRewrite

## Scope
- Baseline: `2e744ada`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/response_rewrite/{mod.rs,plugin.rs}`
- Revalidated blobs: `mod.rs` = `a4cc472b6cb26f99c8e7c4e2cbd0b3690fe6342b`; `plugin.rs` = `dbd1e480720704899b5e0c8c24d3893877668f2f`
- Supporting blobs: resources config = `dbdde5bbe0baaf7aa7863c23d7591ad44496594a`; English guide = `59071e3c21d4d10cf13f683a82829daa9699a0b0`; Chinese guide = `60b9da41617b003fb525843340325c81e6c2f64c`
- Implementation allow-list: `edgion-gateway/src/plugins/http/response_rewrite/plugin.rs`, `edgion-resources/src/resources/edgion_plugins/plugin_configs/response_rewrite.rs`, `docs/en/user-guide/http-route/filters/edgion-plugins/response-rewrite.md`, and `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/response-rewrite.md`
- Forbidden files: `mod.rs`, PluginRuntime/session/route and response-body code, Controller, CRD, other plugins/resources/docs, Cargo, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Traced resources and Controller/Gateway validation parity, PluginRuntime response stage, response-header mutation and ordering, status/header parsing and error handling, streaming body invariants, logs, tests, integration coverage, and documentation.
- Preserve `200..=599`, action order `rename -> add -> set -> remove`, duplicate add semantics, missing rename source behavior, runtime writer-error continuation, fixed diagnostics, response-stage ownership, and the always-streaming upstream response.
- Existing integration tests remain frozen.

## Handoff
- Analyst: independent read-only analysis completed on `10b0584d`; all target/support blobs were unchanged when promoted on `2e744ada`
- Decision: change `plugin.rs` with exact resources and bilingual documentation support; validate and prepare the immutable generation once, reject unsafe framing writes and case-insensitive self-renames before publication, and remove repeated validation/status-log allocation from the response path
- Complexity: moderate/plan-first because shared admission semantics, response framing safety, direct-construction defense, runtime mutation ordering, and documentation must remain coherent
- Plan review: independently approved with no Must Fix or Should Fix; exact 256/257-byte boundary, redacted validation priority, original-value ownership, compile/preparse parity, partial rename semantics, streaming/framing ownership, tests, docs, and four-file scope are sufficient
- Implementation: exact four-file allow-list honored; resources now validate every header position/value with stable redacted priority and framing/self-rename rules, Gateway publishes a prepared generation with static name/status log and direct-invalid defense, runtime action/error ordering remains explicit, and both dedicated guides are synchronized
- Implementer verification: 15 resources and 8 Gateway ResponseRewrite tests; locked Gateway all-target check, format, agent-doc, no-tracing, English-only, diff, and frozen-path guards passed
- Final-review correction: runtime/resources behavior approved; one Should Fix removes contradictory English/Chinese troubleshooting wording that implied case-sensitive rename lookup despite HTTP header-name case insensitivity
- Correction implementation: only the two allowed guides changed; both now state that `from` must identify an existing valid source and matching is case-insensitive
- Final review: independently approved after correction with no remaining Must Fix or Should Fix; validation priority/redaction, framing and self-rename rules, prepared generation ownership, compile sealing, action/failure ordering, partial rename behavior, streaming response ownership, and bilingual documentation are consistent
- Coverage: `plugin.rs` changed; `mod.rs` examined-no-change
- Verification: 15 resources and 8 Gateway ResponseRewrite tests passed in implementation and final review; format, locked workspace check/Clippy, Gateway binary build, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff, and frozen-path guards passed
- No-change candidate: `mod.rs`; response-body handling remains examined-no-change and streaming
- Deferred: D0068 owns a cross-mechanism hop-by-hop response-header mutation policy; D0063 remains limited to rate-limit local-response headers
- Integration: existing frozen status and normal mutation scenarios are sufficient; deterministic validation and unit tests cover this change, so no integration issue is needed
- Commit: pending
