# Add optional weighted or monetary budget enforcement

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P2 |
| Tier | Optional; select for a concrete deployment need |
| Dependencies | [shared-and-hierarchical-token-budgets](shared-and-hierarchical-token-budgets.md), [quota-modes-and-inflight-reservations](quota-modes-and-inflight-reservations.md) |

## AI guide summary

Extend token accounting with optional weighted or monetary limits using existing fixed-point pricing and attempt settlement, without presenting estimates as invoices.

## Requirements and scope

- Choose a clear unit contract for weighted credits and/or money, including input/output/cache/reasoning treatment and provider-specific overlap semantics.
- Freeze the pricing generation used for each attempt and specify stale or missing price behavior.
- Charge retries, incomplete attempts, and observed/estimated/unknown usage through the common settlement path.
- Define integer precision, safe arithmetic, overflow, refunds, reservation estimates, and downstream audit attribution.

### Out of scope

- A complete billing platform, payment collection, or treating estimated gateway cost as the provider invoice.

## Acceptance criteria

- [ ] Budget units and thresholds are unambiguous and arithmetic is deterministic.
- [ ] Declared failure/unknown-price policy cannot silently bypass a configured budget.
- [ ] Raw token evidence and cost quality remain available in the canonical access log.

## Source entry points

- [edgion-gateway/src/ai/accounting/usage.rs](../../../edgion-gateway/src/ai/accounting/usage.rs)
- [edgion-gateway/src/ai/routing/candidate.rs](../../../edgion-gateway/src/ai/routing/candidate.rs)
- [edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs](../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs)

## Task-specific verification

- Mixed models/providers, cache token categories, price-generation changes, all-attempt costs, unknown usage, overflow and idempotent settlement.

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
