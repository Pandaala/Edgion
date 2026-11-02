---
name: conf-sync-watch-endpoint-timeout
description: Use when a reviewer claims the `conf_sync` watch handshake / `client.watch(request).await` has no bounding timeout and blocks indefinitely if the controller stalls — false positive; the endpoint-level tower timeout already bounds the handshake.
knowledge_type: review-evidence
---

# `conf_sync` watch handshake IS bounded by the endpoint-level timeout (h2-grpc-06)

**Status / verdict**: false positive (h2-grpc-06, 2026-05-18)

Finding form: "`conf_sync` watch handshake / `client.watch(request).await` has no bounding timeout / blocks indefinitely if controller stalls".

This is a **false positive**.

`grpc_client.rs:54` sets `.timeout(Duration::from_secs(10))` on the `Endpoint`, which installs a `GrpcTimeout` tower layer in the `Connection` service stack (Tonic 0.12.3 `channel/service/connection.rs:65`). For server-streaming RPCs the layer's `ResponseFuture` races the inner future (which resolves on receiving the response HEADERS) against a 10 s sleep — so `client.watch(request).await` **IS** bounded by 10 s.

After the stream is established, the sleep no longer races subsequent `.message()` calls, so long-lived streams are not affected: the 10 s bound covers the handshake (until response HEADERS arrive), not the lifetime of the stream. That is the correct behavior — a watch stream is supposed to stay open indefinitely once established.

Why the verifier got it wrong: the verifier checked `tonic::Request::set_timeout()` (which sets the `grpc-timeout` header) but **missed the endpoint-level tower timeout** installed by `.timeout(...)` on the `Endpoint`.

Re-reports of this pattern are not accepted. Before re-reporting, verify that `grpc_client.rs` no longer calls `.timeout(...)` on the `Endpoint` (i.e. the endpoint-level tower `GrpcTimeout` layer is genuinely absent) — the `Request::set_timeout()` path alone is not the relevant mechanism here.

## Historical resolution evidence: h2-grpc-06

Verifier missed tower-level `GrpcTimeout`: `Endpoint::timeout(10s)` in `grpc_client.rs:54` installs `GrpcTimeout` in the `Connection` service stack (Tonic 0.12.3 `channel/service/connection.rs:65`). `client.watch(request).await` IS bounded by 10 s; "blocks indefinitely" claim is not accurate. No code change needed.

## Resolution references

- `h2-grpc-06` — 2026-05-18 (❌ false positive).


## Tonic 0.14 migration source check

The unified 0.14.9 dependency still installs `GrpcTimeout` in
`transport/channel/service/connection.rs` with the configured endpoint timeout.
The earlier 0.12.3 analysis above remains historical evidence for the same
headers/stream-establishment boundary; it is not proof that consuming a complete
unary response body is bounded. Keep the explicit complete-RPC deadline and the
`edgion-gateway/tests/conf_sync_failover.rs` stalled-body regression. Migration
runtime verification is recorded with the dependency update, not inferred from
this source check.
