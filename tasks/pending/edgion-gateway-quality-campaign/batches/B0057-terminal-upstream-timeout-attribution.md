# B0057 — Terminal Upstream Timeout Attribution

## Scope

- Baseline: `535af96d2f790c6596060cc1bcfef00998c0e076`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned code file: `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs`.
- Allowed knowledge file: `skills/04-review/h2-grpc/regression-guards/timeout-honored.md`.
- Existing `edgion-tests/**` files, adjacent proxy callbacks, and protected user paths remain forbidden.
- Exact commit allow-list: the two technical files above plus B0056/B0057 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Current-HEAD analysis verdict: `change`; implementation is plan-first because it changes terminal error attribution.
- The final proxy stage currently excludes every 504 from attempt-error append, conflating a real upstream HTTP 504, an already-recorded connect/TLS timeout, and an otherwise-unrecorded terminal upstream read/write timeout.
- Only terminal upstream `ReadTimedout`/`WriteTimedout` should gain a static attempt error kind at this stage, and exact kinds already stamped by an earlier callback must not be duplicated.
- When the captured `PeerTimeoutSource` is `RequestDeadline` or `GrpcDeadline`, add the corresponding canonical request diagnostic; `BackendRequest` and `None` must not fabricate deadline diagnostics.
- Preserve downstream 408/499 classification, real HTTP 504 handling, connect/TLS attribution, wire status/body, HTTP/gRPC rendering, response commitment, retries, outlier accounting, and observability ownership.

## Plan requirements

1. Use the current attempt's `PeerTimeoutSource` only for upstream read/write timeout diagnostics.
2. Narrow attempt-error append to terminal upstream I/O timeouts and suppress only an exact existing kind.
3. Keep real HTTP 504, connect/TLS timeouts, and downstream errors excluded from new append behavior.
4. Cover request/grpc/backend/none sources, real HTTP 504, terminal I/O append, exact deduplication, and existing downstream boundaries in inline tests.
5. Update the approved timeout regression guard without changing protocol behavior.

## Verification plan

- Focused fail-to-proxy, fail-to-connect, and error-while-proxy tests.
- Full campaign repository gates and frozen-path verification.

## Approved plan and implementation

- An independent plan reviewer approved the two-file design and required exact upstream source/type gates, current-attempt error deduplication only, and context-backed source wiring coverage.
- Request diagnostics now use `PeerTimeoutSource` only for upstream `ReadTimedout`/`WriteTimedout`, mapping request and gRPC deadlines to their canonical diagnostics while leaving `BackendRequest` and `None` unclassified.
- Terminal upstream I/O timeout kinds can now enter the current attempt despite their 504 mapping; only an exact existing attempt kind is suppressed.
- Real upstream HTTP 504, connect/TLS timeouts, downstream 408/499, ordinary error behavior, response commitment, retries, and outlier ownership remain unchanged.
- The timeout regression guard now documents the terminal attribution boundary and exclusions.

## Independent code review

- A fresh reviewer approved the implementation with no Must Fix, Should Fix, or deferred finding.
- The review verified source/type gating, deadline mapping, exact deduplication, downstream precedence, real HTTP/connect/TLS exclusions, behavior preservation, and scope containment.

## Verification

- Focused tests passed: 11 fail-to-proxy, 9 fail-to-connect, and 6 error-while-proxy tests.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only.
- Only the campaign's known Kafka and frozen integration-test clippy warnings plus cargo-deny duplicate/advisory-ignore warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `a78f0e8ede00d46ccd97e5b59869d084f5159c5f`.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
