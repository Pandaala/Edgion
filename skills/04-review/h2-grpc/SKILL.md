---
name: review-h2-grpc
description: Review knowledge for h2-grpc, grouped by decision type.
---

# h2-grpc review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [refactor-h2-grpc-02 — HTTP-status sentinel in gRPC error helpers is intentional](intentional-design/h2-grpc-sentinel-refactor-declined.md)
- [Keep Pingora protocol pumps separate](intentional-design/pingora-protocol-pumps-remain-separate.md)
- [Terminate-driven upstream RST_STREAM is not rate-limited — ACCEPTED-RISK](intentional-design/terminate-upstream-rst-not-rate-limited.md)

## Compatibility constraints

- [h2-grpc-01 — gRPC error responses emit 3 frames instead of spec-compliant single HEADERS+END_STREAM](compatibility-constraints/trailers-only-three-frame.md)

## Regression guards

- [Bounded downstream HTTP/2 startup policy](regression-guards/downstream-h2-startup-policy.md)

- [Terminal gRPC status fields have one writer](regression-guards/terminal-status-field-ownership.md)
- [Abandoned Request Bodies Have a Distinct Terminal Event — FIXED](regression-guards/abandoned-request-body-terminal-event.md)
- [Custom terminal 101 normalized before dispatch](regression-guards/custom-terminal-101-normalized-before-dispatch.md)
- [h2-grpc-07 — `fed_client` connect timeout added](regression-guards/fed-client-connect-timeout.md)
- [h2-grpc-02 — `grpc-message` percent-encoding in `send_grpc_error_three_frame`](regression-guards/grpc-message-encoding.md)
- [gRPC-Web is independent of frontend HTTP/2 enablement, including HTTP/1-only integration coverage](regression-guards/grpc-web-independent-of-frontend-http2.md)
- [H2 ci contract enforcement out of scope](regression-guards/h2-ci-contract-enforcement-out-of-scope.md)
- [H2 dependency baseline tests version robust](regression-guards/h2-dependency-baseline-tests-version-robust.md)
- [H2 downstream request-body reads are bounded by `read_timeout` — FIXED](regression-guards/h2-downstream-body-read-timeout.md)
- [H2 wire observer GOAWAY ceiling and read-terminal poisoning](regression-guards/h2-end-stream-wire-observer.md)
- [H2 local reset invalidates shared evidence](regression-guards/h2-local-reset-invalidates-shared-evidence.md)
- [H2 proxy cache reuse harness consumers](regression-guards/h2-proxy-cache-reuse-harness-consumers.md)
- [H2 shutdown connection not allocatable](regression-guards/h2-shutdown-connection-not-allocatable.md)
- [h2 Terminal-Trailer Decoder Limitation — ACCEPTED-RISK](regression-guards/h2-trailer-decoder-upstream-limitation.md)
- [H2 writer capacity stall after response](regression-guards/h2-writer-capacity-stall-after-response.md)
- [Negative tests need out of band observation](regression-guards/negative-tests-need-out-of-band-observation.md)
- [h2-grpc-03 — `grpc-timeout` establishment/I/O bounds, propagation and limits](regression-guards/timeout-honored.md)
- [Trailer batch latch cache completion](regression-guards/trailer-batch-latch-cache-completion.md)
- [Trailer done terminal body dispatch](regression-guards/trailer-done-terminal-body-dispatch.md)

## Review evidence

- [gw-plugin-acl-rl-01 — `PluginSession::shutdown()` needs no `is_http2()` gate](review-evidence/plugin-session-shutdown-h2-no-gate.md)
