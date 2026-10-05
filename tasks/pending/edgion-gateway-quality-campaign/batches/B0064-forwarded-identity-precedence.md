# B0064 — Forwarded Identity Precedence

## Scope

- Baseline: `30ba94f610e804a48696d21ff2c85303c1c08d34`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned code file: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`.
- Approved knowledge file: `skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md`.
- Allowed tests: inline tests in the owned code file only.
- Existing `edgion-tests/**`, adjacent proxy/trace files, and protected user paths remain forbidden.
- Exact commit allow-list: the owned code and knowledge file plus B0063/B0064 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; implementation is plan-first because a valid RequestId configuration can collide with the proxy-owned RemoteIp identity header.
- The hook currently publishes canonical XFF and the configured RemoteIp header, then lets RequestId reconciliation overwrite or remove that same name.
- `x-real-ip` is both a valid custom RequestId header and the default RemoteIp header, so the current order can forward attacker-controlled correlation data instead of `ctx.request_info.remote_ip`.
- Reconcile RequestId after plugins but before the canonical forwarded-header publication, preserving identity-header final ownership without adding another mechanism.

## Plan requirements

1. Canonical XFF and the configured RemoteIp header remain the final proxy-owned identity writes.
2. Preserve ordinary W3C/custom RequestId forwarding when names do not collide.
3. Preserve deferred credential validation/materialization, gRPC timeout, Host/authority, request framing, AI dispatch, WebSocket/H2 behavior, retry idempotence, and error precedence.
4. Add no log, metric, diagnostic, allocation, parser, or header-management mechanism.
5. Record the collision precedence accurately in the approved RequestId lifecycle page.

## Verification plan

- Full-hook collision regression plus ordinary RequestId/trace tests.
- Full campaign repository gates and frozen-path verification.

## Implementation and review

- Moved infallible RequestId reconciliation before canonical forwarded-header publication while preserving every fallible preparation step and AI dispatch ordering.
- Added a real compiled-RequestId full-hook regression covering a default `x-real-ip` collision, distinct custom/W3C propagation, canonical XFF/RemoteIp, fresh retry clones, and unchanged diagnostics/access-log state.
- Updated the approved lifecycle page with exact upstream-wire collision precedence and current validator boundaries.
- Independent plan and final code reviews approved the change with no remaining finding.

## Verification

- All 14 upstream-filter, 28 request-trace, and 8 resource RequestId tests passed.
- Full repository gates and frozen-path verification passed; only the known unrelated warnings remain.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `f1d3cff1e2280d48b3f6afbf73dca7c56b6a6375`.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
