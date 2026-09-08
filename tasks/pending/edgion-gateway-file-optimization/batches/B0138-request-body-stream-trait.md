# B0138 — Request Body Stream Trait

## Scope

- Baseline: `2fffc7049cb58f8a3831e7a328d57453935b5485`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/traits/request_body_stream.rs`
- Baseline blob: `77d843d58f36a30c8401f4eb3063974913f26cb6`
- Proposed supporting allow-list:
  - `edgion-gateway/src/plugins/http/ext_proc/streamed.rs`
  - `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- Supporting blobs at promotion:
  - `edgion-gateway/src/plugins/http/ext_proc/streamed.rs`: `9c042540ad1248799a53b445d20fcfa641af148d`
  - `edgion-gateway/src/plugins/runtime/session_adapter.rs`: `10ad83813330c300b0cf34234d8ed95adc74910e`
- Implementation scope: exact three-file allow-list above; only the owned trait file gains coverage

## Handoff

- Analysis: `b0137_traits_facade_analysis` independently audited the stream trait, both implementations, driver, context/install paths, BodyStorage boundary, ExtProc chunk flow, response writing and related tests. Decision: change; simple API cleanup.
- Value: `RequestBodyStreamHandler::name` is a required vtable method with no caller anywhere in the workspace. Logs and diagnostics instead come from `take_log`, `ChunkVerdict` and `take_pending_outcome`. Removing the dead method reduces the trait surface and meaningless implementation boilerplate without runtime behavior or object-safety changes.
- Proposed change: remove the trait method/documentation and the two implementation stubs from ExtProc and the session-adapter test. Supporting files do not receive campaign coverage from this batch.
- Preserved contracts: serial awaited chunk backpressure, Complete/Abandoned events, non-replayable latch, one canonical BodyStorage, native WAF/observer/mirror/upstream order, bounded dynamic reply, response streaming and no new observability.
- Freshness: promoted at `e4960f34`; all three allowed blobs match the analysis state.
- Implementation: removed the unused required `name` method and its inaccurate diagnostic documentation from the stream trait, plus the only production and test implementation stubs. No runtime expression or observable path changed; supporting files remain pending their own full campaign coverage.
- Review: `b0135_final_review` independently approved with no Must Fix or Should Fix. It verified there are exactly two implementations and no calls, object safety remains intact, ExtProc identity/diagnostics remain owned by PluginLog/outcome/verdict paths, and chunk ordering, backpressure, retry latch, body storage, terminal events, cancellation and response streaming are untouched.
- Verification: ExtProc streamed tests passed 16/16 and both exact session-adapter installation/freeze tests passed 1/1. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,090-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `230c9346`; local only, never pushed by the campaign.
