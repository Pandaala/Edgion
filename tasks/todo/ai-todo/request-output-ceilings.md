# Enforce explicit per-request output ceilings

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

Introduce an output-limit policy distinct from defaults and estimates, applying it consistently before reservation and provider dispatch.

## Requirements and scope

- Define defaults versus enforceable ceilings, reject versus explicit clamp behavior, missing bounds, and model/provider capability differences.
- Apply the same resolved effective bound to admission/reservation, cost estimation, conversion, and native dispatch.
- Specify reasoning/output token semantics and endpoints for which a reliable provider-enforced bound is unavailable.
- Compile policy values before the hot path and retain request-stable policy generations.

### Out of scope

- Claiming an estimated token count is a strict monetary upper bound or silently overriding business parameters.

## Acceptance criteria

- [ ] A client cannot exceed the configured policy ceiling on a supported operation.
- [ ] defaults.maxOutputTokens remains documented as a default, not a hard limit.
- [ ] Unknown or unsupported bound semantics follow the declared policy without silently bypassing enforcement.

## Source entry points

- [edgion-gateway/src/ai/runtime/dispatch.rs](../../../edgion-gateway/src/ai/runtime/dispatch.rs)
- [edgion-gateway/src/ai/conversion/chat_to_anthropic/request.rs](../../../edgion-gateway/src/ai/conversion/chat_to_anthropic/request.rs)
- [edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs](../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs)

## Task-specific verification

- Missing/excessive/conflicting fields, native and converted API dialects, reservation alignment, provider defaults, and hot reload.

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
