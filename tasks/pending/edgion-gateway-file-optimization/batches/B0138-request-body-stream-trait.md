# B0138 — Request Body Stream Trait

## Scope

- Baseline: `2fffc7049cb58f8a3831e7a328d57453935b5485`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/traits/request_body_stream.rs`
- Baseline blob: `77d843d58f36a30c8401f4eb3063974913f26cb6`
- Proposed supporting allow-list:
  - `edgion-gateway/src/plugins/http/ext_proc/streamed.rs`
  - `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- Implementation scope: not approved until promoted and revalidated

## Handoff

- Analysis: `b0137_traits_facade_analysis` independently audited the stream trait, both implementations, driver, context/install paths, BodyStorage boundary, ExtProc chunk flow, response writing and related tests. Decision: change; simple API cleanup.
- Value: `RequestBodyStreamHandler::name` is a required vtable method with no caller anywhere in the workspace. Logs and diagnostics instead come from `take_log`, `ChunkVerdict` and `take_pending_outcome`. Removing the dead method reduces the trait surface and meaningless implementation boilerplate without runtime behavior or object-safety changes.
- Proposed change: remove the trait method/documentation and the two implementation stubs from ExtProc and the session-adapter test. Supporting files do not receive campaign coverage from this batch.
- Preserved contracts: serial awaited chunk backpressure, Complete/Abandoned events, non-replayable latch, one canonical BodyStorage, native WAF/observer/mirror/upstream order, bounded dynamic reply, response streaming and no new observability.
- Review: pending promotion, implementation and fresh final review.
- Verification: focused ExtProc streamed and session-adapter installation tests plus full gates.
- Commit: pending local batch commit; never push.
