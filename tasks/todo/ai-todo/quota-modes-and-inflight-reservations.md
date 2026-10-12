# Define low-cost cumulative quotas and optional inflight reservations

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-10-12 |
| Status | pending |
| Type | feature |
| Priority | P1 |
| Tier | Optional; select for a concrete deployment need |
| Dependencies | [request-output-ceilings](request-output-ceilings.md), [shared-and-hierarchical-token-budgets](shared-and-hierarchical-token-budgets.md) |

## AI guide summary

Separate request admission, usage settlement, and stream interruption semantics; add strict reservation only as an explicit mode with documented limits.

## Requirements and scope

- Retain a low-cost cumulative mode and explicitly describe concurrency overshoot and settlement cost.
- Separate stop-admitting behavior from optional mid-stream termination, throttle, and model downgrade.
- For reservation mode, define estimated input, bounded output, atomic reserve/reconcile, all-attempt charging, expiry, crashes, unknown usage, and dependency failure.
- Reuse shared Redis scripts and lifecycle rather than conflating credential limits with tenant budgets.
- Do not promise zero overshoot without an established upper bound, including multimodal/reasoning usage.

### Out of scope

- Per-token Redis calls, implicit model changes, or a universal strict-money guarantee.

## Acceptance criteria

- [ ] Each mode has an explicit accuracy, concurrency, Redis-operation, and failure contract.
- [ ] Quota omission still creates no quota Redis work or worker.
- [ ] Reserve/settle paths are idempotent and unknown consumption never becomes an unexplained zero.
- [ ] Supported clients receive appropriate native errors when enforcement interrupts a committed stream.

## Source entry points

- [edgion-gateway/src/ai/quota/mod.rs](../../../edgion-gateway/src/ai/quota/mod.rs)
- [edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs](../../../edgion-gateway/src/plugins/common/redis_rate_limiter/scripts.rs)
- [edgion-gateway/src/ai/runtime/attempt_runtime.rs](../../../edgion-gateway/src/ai/runtime/attempt_runtime.rs)
- [edgion-gateway/src/ai/accounting/finalize.rs](../../../edgion-gateway/src/ai/accounting/finalize.rs)

## Task-specific verification

- Concurrent admission, multiple attempts, retries, windows, duplicate finalization, disconnect, missing usage, Redis outage, expiry, and optional stream termination.

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
