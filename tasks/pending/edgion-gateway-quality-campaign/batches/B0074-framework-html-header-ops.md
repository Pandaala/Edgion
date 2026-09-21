# B0074 — Framework HTML Header Operations

## Scope

- Baseline: `7d189cdf`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/runtime/server/error_response.rs`.
- Approved support file: `edgion-gateway/src/ctx.rs`.
- Existing `edgion-tests/**`, sticky-session/upstream headers, protected user paths, and all other files remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `complex`, so implementation is plan-first.
- `send_html_error` is the only first-response terminal writer that does not consume request-stage `pending_response_header_ops`.
- Safe queued mutations are therefore lost on framework/backend HTML errors and bare terminate fallback even though HTTP plugin replies, response-stage replies, body errors, direct writes, and gRPC errors retain them.
- Existing coverage proves stale framing cannot suppress the body but does not justify dropping safe operations.

## Required invariants

- Apply gateway/server defaults, destructively drain the queue once, reconcile the canonical trace ID, then reassert fixed HTML media/framing invariants.
- Preserve safe queued headers while removing queued `Content-Length` and `Transfer-Encoding`; keep current close/EOS framing rather than adding a new length policy.
- Preserve final-response ownership/status, fixed generic fail-close bodies, write order, HTTP/1 shutdown, HTTP/2 EOS, access-log diagnostics, redaction, and observability restraint.
- Add no log, metric, body detail, sticky-session carry-over, new mechanism, or protocol/schema change.

## Verification plan

- Independently approve the exact implementation plan before edits.
- Strengthen the live HTML session test with safe and conflicting queued operations, exact fixed response assertions, and queue-consumption proof.
- Run focused error-response tests, relevant local-reply tests, full campaign gates, and frozen-path verification.

## Deferred findings

- None.

## Outcome

- Framework HTML errors now reuse the canonical queued response-header mechanism after winning final-response ownership.
- The writer preserves safe queued operations, reconciles canonical trace identity, removes conflicting framing, and reasserts the fixed HTML media type before the unchanged header/body/H1 shutdown sequence.
- The live HTTP/1 regression proves single application, destructive queue consumption, canonical headers, EOF, and exact unchunked body bytes.
- Independent plan and final reviews approved the change with no deviations, corrections, or deferred findings.
- Source commit: `159a2a8d` (`Apply queued headers to framework errors`).

## Verification

- Framework error-response tests: 14 passed.
- Session-adapter local-reply tests: 11 passed.
- Request-ID tests: 5 passed.
- `cargo fmt --all -- --check`, workspace check and Clippy, agent-doc, SSA, metrics, Pingora source, Gateway API transform, no-tracing, cargo-deny, English-only, diff, and frozen-path gates passed.
- Clippy emitted only the known Kafka `err_expect` and frozen integration DSL `redundant_async_block` warnings; cargo-deny emitted only the accepted duplicate/advisory-ignore warnings.

## Handoff

- Final ordered anchor: `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` (B0075), after current-HEAD revalidation and mandatory plan review for its queued EdgionTls serving-chain fix.
