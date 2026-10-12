# Complete the supported Chat-to-Claude conversion subset

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

Make the existing Chat-to-Claude adapter a clearly supported and practically usable common subset while retaining explicit rejection of nonportable semantics.

## Requirements and scope

- Define and validate common text, system/developer handling, function tools, stop controls, output limits, and supported sampling parameters.
- For each added field, state exact semantic mapping or explicit unsupported behavior; do not silently remove important fields.
- Preserve the supported JSON subset for omitted/false `stream` and SSE for `stream: true`, including finish reasons, tool IDs/arguments, usage, error mapping and framing. Reuse the existing bounded unary converter owner and its admission/precommit failure machinery; do not duplicate it or silently promote JSON to SSE.
- Document images, strict output, thinking, citations, and other advanced features as supported or native-only according to evidence.

### Out of scope

- Full feature equivalence across vendors or implementation of provider-hosted tools in the gateway.

## Acceptance criteria

- [ ] The public compatibility matrix matches actual parser/converter behavior.
- [ ] Common SDK requests within the subset work without surprising default-only parameter restrictions.
- [ ] Unsupported semantics fail clearly and do not trigger an incompatible fallback.

## Source entry points

- [edgion-gateway/src/ai/conversion/chat_to_anthropic/request.rs](../../../edgion-gateway/src/ai/conversion/chat_to_anthropic/request.rs)
- [edgion-gateway/src/ai/conversion/chat_to_anthropic/response.rs](../../../edgion-gateway/src/ai/conversion/chat_to_anthropic/response.rs)
- [edgion-tests/src/client/suites/edgion_backend/anthropic_provider.rs](../../../edgion-tests/src/client/suites/edgion_backend/anthropic_provider.rs)

## Task-specific verification

- SDK JSON/SSE fixtures, text/tool interleaving, tool-history graph cases, fragmented tool arguments, parameter boundary/unsupported cases, zero-dispatch unsupported semantics, SSE errors and usage reconciliation. Reuse the existing bounded unary converter resource/commitment regressions for added fields.

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
