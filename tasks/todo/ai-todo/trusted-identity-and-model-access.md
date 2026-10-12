# Bind trusted identities to AI model and provider access

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

Build a safe reusable governance profile on existing authentication and condition mechanisms, then add only the missing identity/access-policy contract.

## Requirements and scope

- Publish safe JWT/OIDC/KeyAuth identity composition with explicit required claims, header sanitization, ordering, and missing-dimension behavior.
- Reuse the canonical identity/key/condition machinery; extend it only if existing composition is insufficient.
- Define caller-to-provider/model allow/deny policy with defaultAction and consistent native/converted enforcement.
- Retain client model ownership; distinguish model catalog visibility from invocation permission.
- Define SDK-facing authentication and make gateway credentials distinct from upstream provider credentials.

### Out of scope

- A second authentication system, ad-hoc per-provider policy engines, or changing global claim projection semantics without evidence.

## Acceptance criteria

- [ ] Client-supplied identity headers cannot override the configured trusted principal in the reference profile.
- [ ] Every required identity dimension is validated before quota/access decisions.
- [ ] Unauthorized model/provider requests are rejected before provider dispatch in both modes.
- [ ] Existing generic authentication capabilities are reused and documented accurately.

## Source entry points

- [edgion-resources/src/common/key_accessor.rs](../../../edgion-resources/src/common/key_accessor.rs)
- [edgion-gateway/src/plugins/runtime/conditions/extract.rs](../../../edgion-gateway/src/plugins/runtime/conditions/extract.rs)
- [edgion-gateway/src/plugins/http/common/auth_common.rs](../../../edgion-gateway/src/plugins/http/common/auth_common.rs)
- [edgion-gateway/src/plugins/http/common/mod.rs](../../../edgion-gateway/src/plugins/http/common/mod.rs)
- [edgion-resources/src/resources/edgion_backend.rs](../../../edgion-resources/src/resources/edgion_backend.rs)

## Task-specific verification

- Spoofed/missing/unsafe claims, plugin order, user/team/tenant identity, model visibility versus permission, unknown models, and native/converted parity.

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
