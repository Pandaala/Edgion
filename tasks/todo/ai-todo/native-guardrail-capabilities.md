# Extend guards through explicit native protocol capabilities

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P1 |
| Tier | Optional; select for a concrete deployment need |
| Dependencies | Existing shared AI runtime; revalidate before starting |

## AI guide summary

Preserve existing guard behavior and add native protocol support only through explicit semantic extraction and transformation capabilities.

## Requirements and scope

- Publish request and response guard capability matrices per operation/protocol.
- Add verified native text/span extraction and safe masking without altering opaque reasoning/tool signatures.
- Reuse canonical semantic records and RawWindow for supported SSE protocols; define unavailable semantic data explicitly.
- Preserve body-generation ordering, permit/timeout behavior, first-window response-head holds, and native error/terminal envelopes.

### Out of scope

- Automatic semantic guarding of opaque traffic, complete-response JSON inspection, or a second response-window implementation.

## Acceptance criteria

- [ ] Unsupported guard/protocol combinations are explicit and cannot silently claim protection.
- [ ] Disabled guards allocate no evaluator/window state.
- [ ] Supported replacement/rejection preserves native framing and streaming backpressure.

## Source entry points

- [edgion-gateway/src/ai/guards/request/plugin.rs](../../../edgion-gateway/src/ai/guards/request/plugin.rs)
- [edgion-gateway/src/ai/guards/request/spans.rs](../../../edgion-gateway/src/ai/guards/request/spans.rs)
- [edgion-gateway/src/ai/guards/semantic/plugin.rs](../../../edgion-gateway/src/ai/guards/semantic/plugin.rs)
- [edgion-gateway/src/sse/window.rs](../../../edgion-gateway/src/sse/window.rs)

## Task-specific verification

- Protocol-specific span cases, mutation conflicts, fragmented SSE, semantic-unavailable events, hold-first-window H1/H2 behavior, evaluator failure and cancellation.

## Independent implementation boundaries

This is future work, not authorization to implement or call a paid provider now.
Revalidate the current source and applicable review knowledge when starting; code
pointers below are investigation entry points, not verified completion claims.

- Edgion is a proxy-layer gateway. Preserve client-owned model choice and native
  payloads. Keep conversion and usage observation explicit; observation defaults off.
- Reuse canonical credentials, auth/key extraction, routing, snapshots, retries,
  deadlines, Redis limits, attempt accounting and final access logging. No agent
  runtime, hosted tool execution, MCP/A2A gateway or new parallel infrastructure.
- Main upstream responses stream. No response BodyStorage, disk spool, unbounded
  aggregation or second SSE window. Semantic windows use bounded RawWindow. The
  existing selected unary Chat-to-Anthropic converter has its own strict memory,
  complexity, admission, deadline and precommit-error contract; do not extend that
  exception to another converter without a separate reviewed design.
- Compile policy before request processing. Preserve cancellation cleanup,
  all-attempt accounting and Controller serial workqueue discipline.
- Gateway traffic diagnostics belong to canonical access logs and bounded metrics;
  no request-path tracing, payload/credential logging or identity/model metric labels.
- Any operator-facing schema change needs CRD coverage; internal resolved fields
  retain schema hiding, conf_sync serialization and secret redaction.

## Knowledge and verification

Read [project rules](../../../AGENTS.md), then the [knowledge index](../../../skills/SKILL.md)
and [design rules](../../../skills/00-design-rules.md). Load the relevant original
[AI/backend architecture](../../../skills/01-architecture/05-resources/22-edgion-backend.md),
[review index](../../../skills/04-review/SKILL.md) and
[testing index](../../../skills/05-testing/SKILL.md) on demand. Those project
sources remain authoritative; this task is self-contained as a work specification.

Use a reviewed plan for complex implementation. Add meaningful targeted unit,
integration and hot-reload tests; preserve ordinary HTTP/native/SSE controls and
strict residue. Put unit bodies under the owning crate's tests/unit tree. Run the
applicable project checks and update current capability/user documentation.
Run cargo test --all only when explicitly requested. Paid calls require a concrete
endpoint/model/credential-reference/call-count/budget scope authorized for that run.
