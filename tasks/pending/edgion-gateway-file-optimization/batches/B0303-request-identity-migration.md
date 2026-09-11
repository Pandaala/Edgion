# B0303 — Request Identity Migration

## Scope

- Baseline: `e62956dc`.
- Pipeline state: terminal; changed and committed after independent approval and all gates.
- Active anchor crate: `edgion-gateway`.
- Cohesive targets: `edgion-gateway/src/plugins/http/request_id/mod.rs`, `edgion-gateway/src/request_id_tests.rs`, `edgion-gateway/src/request_trace_id/mod.rs`, and `edgion-gateway/src/request_trace_id/w3c.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- External `79b72bb3` replaced attachable TraceContext with RequestId plus request-owned W3C state and cross-layer tests. These files form one ownership boundary and cannot be assessed safely in isolation.
- The analyst must inspect resource validation, plugin compilation, early/ordinary dispatch, per-reference Arc pinning, listener/Gateway ownership, every final header reconciliation path, bounded state, masking, access-log/OTLP consumers and deterministic tests before deciding change, examined-no-change or defer.

## Analysis result

- Analyst `b0303_request_identity_analysis` read all four targets and traced schema/Controller parity, plugin publication and phased dispatch, listener ownership, every final header writer, access-log/OTLP/mirror/AI/Guardrail consumers, tests and superseded TraceContext history.
- Decision: change, simple. `request_trace_id/mod.rs` can avoid three bounded hot-path costs: transfer an owned generated traceparent directly into `HeaderValue`, lazily materialize the default trace-ID header only when re-enabling a missing value, and skip aggregate-byte scans when no candidate value exists.
- Exact proposed technical allow-list is `edgion-gateway/src/request_trace_id/mod.rs` and `edgion-gateway/src/request_trace_id/w3c.rs`. `plugins/http/request_id/mod.rs` and `request_id_tests.rs` are examined-no-change candidates after cohesive review.
- Preserve W3C bytes, masking, tombstones, first-set/forwarding/echo/caps, final reconciliation, canonical access-log/OTLP ownership and request-pinned Arc behavior. No support, documentation or integration edit is needed.
- Fresh focused verification passed 26 request-trace-id tests and 4 cross-layer request-ID tests. No deferred finding was identified.

## Implementation

- Implementer `b0303_request_identity_implement` added a consuming generated-traceparent accessor and transfers its owned allocation directly into `HeaderValue`.
- Default trace-ID headers are now materialized only when re-enabling a missing implicit value, and aggregate active-byte scans run only when a candidate value exists.
- W3C bytes, validation, masking, tombstones, first-set/forward/echo/caps, final reconciliation, access-log/OTLP ownership and request-pinned Arc behavior are unchanged.

## Independent review

- Reviewer `b0303_request_identity_review`: final LGTM with no Must Fix, Should Fix or deferred finding after the implementer isolated the borrowed traceparent accessor to tests and narrowed the consuming accessor to parent-module visibility.
- Locked `http`/`bytes` conversion paths confirm allocation transfer with equivalent validation; every cap/tombstone/forwarding edge remains enforced. The reviewer passed 14 core, 12 W3C and 4 cross-layer tests.

## Verification

- Focused request-trace-id and RequestId tests passed 30/30; formatting and diff checks passed.
- Applied format plus locked workspace all-target check and Clippy passed. Agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards all passed; Clippy retained only existing warnings.
- Frozen integration files were not modified.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `bf5e3a6e`.
