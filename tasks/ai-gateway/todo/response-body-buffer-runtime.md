# Response-body buffering runtime (read_response_body + DSL resp.body)

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-03 (extracted from the superseded `llm-ai-gateway/01-body-processing-runtime.md`) |
| Status | pending |
| Type | feature |
| Priority | P1 — prerequisite for ExtProc BUFFERED response mode and AI-4 non-streaming guards |
| Related | `docs/superpowers/specs/2026-07-23-body-gate-admission-model-design.md`, `tasks/todo/ext-proc-body-processing.md`, `tasks/todo/ai-gateway/04-guardrails.md` |

## Problem

`read_response_body` still returns `Err(BodyReadError::Unimplemented)` — the
`RequestAdmission::Buffer` arm in `plugins/runtime/session_adapter.rs` (~:1159) has no
chunk-accumulate-then-replay mechanism, because `response_filter` only receives the
downstream Session. Bounded non-streaming response replacement therefore does not exist,
which blocks every buffered response-body consumer (ExtProc response BUFFERED, DSL
`resp.body`, non-streaming response guards).

The streaming path is NOT this task: streaming responses stay chunk-processed via the
`StreamingResponseProcessor` pipeline (see
`docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`). This task is only the
bounded, non-streaming buffered-response mode.

## Inherited residue from `response-body-gate-sync-stage` (closed 2026-08-03)

That task's Finding 2 (a response-stage denial being silently dropped) is fixed and
verified — see
`skills/04-review/architecture/response-stage-denial-must-terminate.md`. Finding 1 was
scoped around response-body gating, which cannot exist until this task lands:

- **Un-reject `resp.body`.** `DslConfig::compile_and_validate`
  (`edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs`) currently
  rejects any script referencing `BuiltinId::RespBody`, and `call_builtin_body` returns a
  fixed `BodyAccess` error. Both enforce the current streaming-only invariant and must be
  lifted together with the docs below.
- **Gate the sync response array.** `add_from_upstream_response_filters`
  (`plugins/runtime/plugin_runtime.rs`) has no `resolve_body_requirement` wrapping step,
  and `edgion_plugin.rs`'s intrinsic lookup has no `BodyStage::Response` arm for `Dsl` at
  all. Both are needed before a response-body consumer can be gated. The array's trait
  (`UpstreamResponseFilter`) is **synchronous** while both existing gates are
  `async_trait`, so this needs the `block_in_place` bridge DSL already uses there —
  tradeoff recorded in
  `skills/04-review/architecture/filter-iteration-control-sync-phase-tradeoff.md`.
- **The gate's fail-close must route through the verified abort path.** A `NotBuffered`
  refusal becomes `ErrResponse { status: 500 }`, which on a gRPC route / real `101` /
  CONNECT tunnel cannot be a local reply. That path now terminates correctly; do not
  reintroduce a log-and-continue branch.
- **`req.body` in `upstreamResponseFilterPlugins` stays as documented** — reads an
  existing capture, never starts late capture, fail-closes when there is none. Rejecting
  it at PREPARSE was considered and declined: validation cannot know whether another
  entry in the composed chain captured the body.
- **Docs to flip** once `resp.body` is supported:
  `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md` (the
  `resp.body` "not supported" section and the `req.body` availability block) and its
  `docs/zh-CN/` mirror.
- **`AGENTS.md` invariant #2** ("the main proxy upstream response is always streaming …
  full-upstream-body APIs must be rejected during configuration validation") is a 🔴
  rule. This task changes it, so the rule text must be amended in the same change —
  otherwise the new code violates the project's own gate on day one.

## Design spike required before implementation

Verify Pingora lifecycle semantics rather than inferring from hook names:

- How response buffering interacts with downstream header commitment (a buffered
  replacement must happen before headers are committed downstream).
- Whether the streaming classification from the phase-1 design
  (`ctx.response_stream_kind`) is the right eligibility switch: a response classified as
  streaming must never be buffered by this mechanism.
- Overflow policy: reject vs. pass-through-unprocessed, and how that composes with the
  R5 rule (`NotBuffered` is unconditionally fail-close for gated consumers).

## Acceptance

- A buffered response policy can replace a non-streaming response before downstream
  commitment, with correct framing (Content-Length recalculated or removed).
- Oversized responses follow the configured policy without unbounded memory growth.
- Streaming responses are never buffered; the gate refuses (fail-close) any buffered
  consumer on a streaming response.
- `read_response_body` returns real outcomes per the R3 five-outcome contract.
