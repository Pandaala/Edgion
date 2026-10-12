# Support explicit shared budget identities and independent quota dimensions

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P1 |
| Tier | Optional; select for a concrete deployment need |
| Dependencies | [trusted-identity-and-model-access](trusted-identity-and-model-access.md) |

## AI guide summary

Extend existing shared-entry token quotas with explicit cross-entry budget identity; keep independently charged user/team/tenant ceilings an optional extension.

## Requirements and scope

- Define a named/explicit budget identity reusable across configured route/plugin entries without breaking existing scope isolation.
- Specify namespace/trust ownership, conflict validation, quota policy generation, and window semantics.
- Allow independent identity selectors per limit only when hierarchical ceilings are required; validate all mandatory dimensions.
- Preserve all-attempt final charging, idempotent updates, and shared Redis limiter ownership.
- Keep alias bucketing explicit; use an aggregate identity cap rather than canonicalizing through the first provider.

### Out of scope

- A second Redis quota implementation or assuming a composite identity key already implements a hierarchy.

## Acceptance criteria

- [ ] Same-budget routes share a bucket and different budgets remain isolated.
- [ ] Reusing the same current entry continues to work without unnecessary new infrastructure.
- [ ] If hierarchy is enabled, one request charges every configured user/team/tenant ceiling independently and handles partial failures explicitly.

## Source entry points

- [edgion-gateway/src/plugins/common/mod.rs](../../../edgion-gateway/src/plugins/common/mod.rs)
- [edgion-gateway/src/ai/quota/runtime.rs](../../../edgion-gateway/src/ai/quota/runtime.rs)
- [edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs](../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs)
- [skills/04-review/ai-routing/regression-guards/identity-model-quota-uses-requested-logical-name.md](../../../skills/04-review/ai-routing/regression-guards/identity-model-quota-uses-requested-logical-name.md)

## Task-specific verification

- Two routes/two entries/two providers, conflicting budget definitions, missing identity dimensions, aliases, multi-limit updates, and multi-gateway concurrency.

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
