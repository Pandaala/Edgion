# B0070 — TLS Identity-Isolation Coverage

## Scope

- Baseline: `ea62dc22cebd5322e392967e0b9bb3e473caf9c3`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/tls/tests.rs`.
- Approved support file: `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_runtime.rs`.
- Approved knowledge file: `skills/04-review/tls/regression-guards/tls-route-stage-isolation.md`.
- Existing `edgion-tests/**` and protected user paths remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `complex`, so implementation is plan-first.
- The existing both-mode route matrix constructs default `TlsContext` identity for terminate and passthrough, so it cannot detect either terminated identity loss or passthrough identity leakage.
- Add a test-only plugin-runtime construction seam and a focused Stage 2 spy/deny test that observes the dispatcher-supplied identity for each mode.

## Required invariants

- The construction seam is test-only and adds no production API, allocation, branch, or behavior.
- Terminated mode passes borrowed negotiated identity and actual mTLS state; passthrough always strips both even if the outer context carries them.
- SNI, canonical direct/remote IPs, listener port, matched Rsc, Stage-2-only dispatch, fail-close behavior, and publication semantics remain unchanged.
- No rustls production assertion, sensitive log value, access-log change, tracing, metric, or observability expansion.

## Verification plan

- Add a local plugin spy/deny test covering both modes and all shared context fields without global state, network, or environment dependencies.
- Rerun TLS module and ClientHello socket tests.
- Run full campaign gates and frozen-path verification.

## Deferred findings

- None.

## Outcome

- Added a test-only runtime constructor and a local Stage-2 spy/deny plugin without changing production behavior or API surface.
- The focused regression proves Terminated mode exposes negotiated TLS identity and mTLS state, Passthrough strips both, and both preserve SNI, canonical peer identities, listener port, and the exact matched resource.
- Independent plan and final reviews approved the change with no required corrections or deferred findings.
- Source commit: `395ccb42` (`Cover TLS route identity isolation`).

## Verification

- Exact identity-isolation regression: 1 passed.
- TLS route module: 80 passed.
- Socket ClientHello regression: 1 passed.
- `cargo fmt --all -- --check`, workspace check and Clippy, agent-doc, SSA, metrics, Pingora source, Gateway API transform, no-tracing, cargo-deny, English-only, diff, and frozen-path gates passed.
- Clippy emitted only the known Kafka `err_expect` and frozen integration DSL `redundant_async_block` warnings; cargo-deny emitted only the accepted duplicate/advisory-ignore warnings.

## Handoff

- Next ordered anchor: `edgion-gateway/src/routes/udp/routes_mgr.rs` (B0071), after revalidating its queued analysis against current HEAD.
