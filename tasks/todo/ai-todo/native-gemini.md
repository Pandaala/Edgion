# Add managed Gemini native access

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P2 |
| Tier | Deferred; select for a concrete deployment need |
| Dependencies | Existing shared AI runtime; revalidate before starting |

## AI guide summary

Add Gemini native model operations with protocol-specific model/auth/usage extraction while preserving provider-native payloads.

## Requirements and scope

- Freeze and implement generateContent and supported streaming operation paths, including model extraction from the URL.
- Implement the selected public Gemini authentication contract and endpoint normalization.
- Preserve content parts, function calls, generation configuration, and native provider fields.
- Observe usageMetadata and provider termination/errors incrementally; specify cache/reasoning token semantics.
- Keep Vertex or other Google-hosted APIs a separate declared hosting/auth profile if added.

### Out of scope

- Automatic support for every Google product or a new Google agent runtime.

## Acceptance criteria

- [ ] A supported Gemini SDK can use the selected native operations through the gateway.
- [ ] Native stream transport and usage semantics match the frozen contract.
- [ ] Resource enum, CRD, Secret resolution, runtime snapshots, documentation, and tests are updated together.

## Source entry points

- [edgion-resources/src/resources/edgion_backend.rs](../../../edgion-resources/src/resources/edgion_backend.rs)
- [edgion-gateway/src/ai/runtime/dispatch.rs](../../../edgion-gateway/src/ai/runtime/dispatch.rs)

## Task-specific verification

- Model URL parsing, native function calling, multimodal byte-preservation, JSON/stream usage, errors, credential hot reload, and H1/H2 as supported.

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
