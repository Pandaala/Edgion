# Expose budget state and reconciliation outcomes for operators

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

Provide bounded production visibility into remaining/reserved/charged/unknown budget state and settlement failures using existing authenticated admin and access-log boundaries.

## Requirements and scope

- Define authorized budget status queries and operator diagnostics with explicit source, freshness, and quality.
- Expose settlement outcomes and supported remediation paths without promising knowledge of unobservable provider consumption.
- Decide whether a durable repair ledger is required by the budget contract before implementing background repair.
- Reuse authenticated admin infrastructure and canonical access logs; provide downstream aggregation guidance for reports.
- Keep identity/model cardinality out of Prometheus labels and redact credentials, prompts, completions, and internal dependency detail.

### Out of scope

- An unsolicited billing dashboard, payment system, or duplicate traffic logging/metrics pipeline.

## Acceptance criteria

- [ ] Operators can distinguish reserved, charged, estimated/unknown, and failed reconciliation states where supported.
- [ ] Queries and repairs respect scope/authorization and cannot double-charge or blindly refund unknown attempts.
- [ ] Documented retention, restart, and dependency-failure limitations match actual guarantees.

## Source entry points

- [edgion-gateway/src/api/mod.rs](../../../edgion-gateway/src/api/mod.rs)
- [edgion-gateway/src/ai/accounting/finalize.rs](../../../edgion-gateway/src/ai/accounting/finalize.rs)
- [docs/en/ai-gateway/observability.md](../../../docs/en/ai-gateway/observability.md)

## Task-specific verification

- Admin authorization, freshness, pagination/cardinality bounds, redaction, duplicate repair, crash/restart and Redis failure scenarios.

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
