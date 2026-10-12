# Add a selected Bedrock endpoint and authentication profile

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

Choose and implement one explicit Bedrock endpoint family rather than treating Bedrock as a single universal model protocol.

## Requirements and scope

- Choose the initial endpoint family based on supported model/API requirements; document its model IDs, regions, paths, authentication, and streaming framing.
- Prefer reuse of already-supported API formats when a verified Bedrock endpoint provides them.
- For an IAM/SigV4 profile, define credential sources, expiry/rotation, signing ownership, canonical host/path/body, retry signing, and sensitive-data handling.
- For an API-key profile, define endpoint compatibility and credential isolation explicitly.
- Implement bounded event decoding only when the selected family requires it; reuse common usage and attempt accounting.

### Out of scope

- Simultaneous blanket support for Converse, InvokeModel, every compatibility endpoint, and Bedrock agent services.

## Acceptance criteria

- [ ] The selected family is fully described and exercised with a supported client.
- [ ] Authentication remains correct across path/host preparation, retries, refresh, and hot reload.
- [ ] Unsupported families fail with explicit configuration/runtime behavior rather than a misleading provider-wide support claim.

## Source entry points

- [edgion-resources/src/resources/edgion_backend.rs](../../../edgion-resources/src/resources/edgion_backend.rs)
- [edgion-gateway/src/ai/connectors/provider.rs](../../../edgion-gateway/src/ai/connectors/provider.rs)

## Task-specific verification

- Authentication/signing golden cases where applicable, region/model URI handling, credential expiration, streaming fragmentation, errors, and usage settlement.

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
