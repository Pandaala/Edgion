# B0063 — DirectEndpoint AppProtocol

## Scope

- Baseline: `00eb71e2ad4b739cfaf1ac2fe87ea27e19f8b1e1`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`.
- Approved support files:
  - `edgion-gateway/src/backends/validation.rs`
  - `edgion-gateway/src/plugins/http/direct_endpoint/plugin.rs`
  - `edgion-gateway/src/ctx.rs`
- Allowed tests: inline crate-local tests in the four approved runtime files only.
- Existing `edgion-tests/**`, documentation, adjacent backend/plugin files, and protected user paths remain forbidden.
- Exact commit allow-list: the four runtime files plus B0062/B0063 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; implementation is plan-first because it crosses validated Service-section state, request context, retry persistence, peer construction, and upstream wire-protocol selection.
- DirectEndpoint validates the exact Service section that authorizes the socket but currently carries only address, slot, and TLS policy into request state.
- The normal Service path reads that section's `appProtocol` and applies the canonical backend protocol helper; the DirectEndpoint path drops it and therefore constructs an HTTP/1 peer for an H2C-only section.
- Carry the already-published immutable `Option<Arc<str>>` through the existing validated snapshot and preset, then reuse `parse_backend_app_protocol` and `apply_backend_app_protocol` during direct peer construction.

## Plan requirements

1. Preserve exact-section validation for named and numbered ports; never perform a second live ServiceStore lookup.
2. Preserve the captured protocol value across retries with the existing request-pinned preset.
3. H2C must produce an H2-only direct peer; WebSocket must remain H1-only; absent or unknown values retain current default behavior.
4. Preserve TLS/SNI policy, slot identity, timeout/deadline handling, address validation, connection reuse isolation, and access-log source tagging.
5. Add no parser, protocol enum, allocation beyond immutable Arc refcounting, log, metric, or tracing path.

## Verification plan

- Focused DirectEndpoint validation, preset/retry, and direct peer protocol tests.
- Full campaign repository gates and frozen-path verification.

## Implementation

- Captured the authorized Service section's published `Option<Arc<str>>` in `ValidatedDirectEndpoint` and moved it into the retry-pinned `DirectEndpointPreset` without a string copy or live lookup.
- Applied the canonical backend app-protocol parser/helper after final address and TLS/plain peer construction and before common attempt/timeout initialization.
- Direct H2C peers are H2-only; WebSocket peers remain H1-only with canonical discovery metadata; absent and unknown values retain the existing default, and protocol metadata never enables TLS.
- Preserved section/port isolation, backend slot, TLS/SNI and pool identity, address safety, retries, deadlines/timeouts, access-log source, and the single-pointer outer context layout.

## Independent reviews

- The independent plan reviewer approved the Arc ownership, 80-byte boxed jump-target layout, helper ordering, protocol/TLS separation, and real same-context retry test design.
- A fresh final reviewer returned LGTM with no Must Fix, Should Fix, or deferred finding after inspecting every initializer, pinned Pingora semantics, and the production tests.

## Verification

- All 3 validation, 11 DirectEndpoint plugin, 7 context jump, and 96 upstream-peer tests passed.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only.
- Only the existing Kafka `err_expect`, frozen integration-test redundant-async, and cargo-deny duplicate/advisory-ignore warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending; it will be backfilled by the next batch.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
