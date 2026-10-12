# Add the limited Chat-to-Bedrock route

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P2 |
| Tier | Deferred; select for a concrete deployment need |
| Dependencies | [bedrock-endpoint-authentication](bedrock-endpoint-authentication.md) |

## AI guide summary

Expose the selected Bedrock model/API profile through the unified Chat entry without expanding into a provider-wide conversion promise.

## Requirements and scope

- Reuse the selected family's compatibility format when it satisfies the verified subset.
- A verified Chat-compatible endpoint may forward native JSON/SSE without an Edgion response converter. If translation is needed, restrict it to the declared text/function-tools SSE, error and usage mappings; reject non-streaming conversion before dispatch.
- JSON-to-JSON conversion requires a separately reviewed future contract and must not retain the complete main response; it is not authorized by this deferred task's current scope.
- Ensure final provider bytes and headers are prepared before signing and are re-prepared safely on retries.
- Bind fallback eligibility to model/API/auth capabilities; preserve exact shared quota and attempt accounting.

### Out of scope

- Every Bedrock model dialect, endpoint family, agent API, and hosted tool service.

## Acceptance criteria

- [ ] The unified route works for the selected models and declared common subset.
- [ ] Native hosting support, compatibility support, and translated support are separately documented.
- [ ] Authentication and accounting remain correct on failure/retry/cancellation.

## Source entry points

- [edgion-gateway/src/ai/runtime/dispatch.rs](../../../edgion-gateway/src/ai/runtime/dispatch.rs)
- [edgion-gateway/src/ai/runtime/attempt.rs](../../../edgion-gateway/src/ai/runtime/attempt.rs)

## Task-specific verification

- Supported client fixtures, event-framing fragmentation where needed, tool round trip, signing-after-transformation where applicable, and incompatible fallback rejection.

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
