# B0188 — HTTP Upstream Peer

## Scope

- Analysis baseline: `848e96e9621ab2765ded74b24135d6ccd943a51d`; revalidated at record-only HEAD `22d0e0ff`
- Target blob: `a87020f0eaae4f0c61a44723edbfff57613ccb17`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; complex because the target owns a hot URI/protocol boundary. `finalize_request_uri` sets `uri_finalized` before the optional URI write, contrary to the mandatory single-write contract. Several comments also incorrectly group DirectEndpoint with ExternalJump as lacking a Service key/policy even though DirectEndpoint captures authority, timeout, and retry-budget policy.
- Frozen technical scope: `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`, including inline tests only.
- Required preservation: parsing failure leaves URI/latch unchanged; normalized/raw/query behavior and idempotence remain stable; DirectEndpoint Service policy/TLS/authority/retry ownership, ExternalJump SSRF, all HTTP/gRPC/AI selection, deadlines, pooling, outliers, streaming, and access-log fields remain unchanged.
- Deferred record: `D0106` owns backendRef-scoped upstream-selection plugin semantics; no partial late-dispatch change is authorized.
- Baseline target tests: 88/88 passed.
- Previous batch: `22d0e0ff`; never push.

## Implementation Plan

1. Preserve URI choice/parsing, perform any differing URI write, then immediately set the finalization latch; equal URI still latches and parse failure mutates neither value.
2. Correct only DirectEndpoint/ExternalJump Service-policy and endpoint-breaker ownership comments.
3. Add live H1 tests for normalized write/query/latch, already-finalized no-op, equal-URI latch, and malformed URI rollback.
4. Keep the diff to this one file and stop on any routing, TLS, retry, SSRF, streaming, observability, or integration change.
5. Run focused proxy-peer/proxy-http tests and all campaign repository gates.

## Outcome

- Implementation: `finalize_request_uri` now writes any differing parsed URI before immediately setting its latch; equal URIs still latch, parse failures change neither value, and already-finalized retries remain no-ops. Four live H1 tests cover write/query, equality, idempotence, and rollback. DirectEndpoint/ExternalJump comments now match tested Service-policy and endpoint-breaker ownership.
- Review: fresh final review inspected working blob `50add5a706084433e0d0f7dbc409cd3ece48b9e4` and returned LGTM with no Must Fix or Should Fix item. It confirmed no intervening fallible operation, preserved query/raw behavior, accurate policy comments, and the separate `D0106` classification.
- Verification: URI tests passed 4/4 and the complete proxy-http suite passed 253/253. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and frozen-path guards passed.
- Commit: pending; never push.
