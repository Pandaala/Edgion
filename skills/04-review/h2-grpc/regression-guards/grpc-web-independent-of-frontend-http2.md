---
name: grpc-web-independent-of-frontend-http2
description: Use when reviewing gRPC-Web module registration or proposals to gate it on frontend HTTP/2; gRPC-Web over HTTP/1.1 remains available independently of h2c/ALPN enablement.
status: fixed
knowledge_type: regression-guards
---

# gRPC-Web is independent of frontend HTTP/2 enablement

## Conclusion

Every Edgion HTTP listener registers exactly one Pingora `GrpcWeb` module,
regardless of the listener's `enable-http2` annotation. That annotation controls
frontend h2c/ALPN and the native-gRPC HTTP/2 requirement; it does not control
whether an HTTP/1.1 client may use gRPC-Web.

The registered `GrpcWebBridge` is inert by default. Before Pingora's module
filter can rewrite headers, Edgion runs the canonical protocol detector against
the raw request and initializes the bridge only when that detector selects
`grpc-web`. Native gRPC and ordinary HTTP leave the bridge disabled. An exact
WebSocket upgrade retains its existing precedence even if the request also
carries a gRPC-Web Content-Type, so the bridge remains disabled.

## Root cause and correction

`pg_init_downstream_modules` previously registered `GrpcWeb` only when
`EdgionHttpProxy.enable_http2` was true. An HTTP/1-only listener therefore had no
bridge context for an otherwise valid HTTP/1.1 gRPC-Web request, even though the
bridge exists specifically to translate gRPC-Web to the upstream HTTP/2 gRPC
exchange.

Module registration now depends only on the resolved response-compression level:
the shared registration seam always adds `ResponseCompressionBuilder` and
`GrpcWeb`. It never reads the frontend HTTP/2 flag. Pingora rejects duplicate
module context types while building the typed context, preserving the
exactly-one invariant.

## Ordering and preserved boundaries

Pingora calls Edgion's `early_request_filter` before its downstream module
`request_header_filter` chain, and calls Edgion's ordinary `request_filter`
after that chain. Edgion therefore freezes the canonical raw protocol
classification and initializes `GrpcWebBridge` before Pingora rewrites a
gRPC-Web Content-Type to native gRPC. The later metadata-stage detector is a
no-op once that classification exists, preventing an HTTP/1.1 gRPC-Web request
from being misclassified as native gRPC and rejected by the frontend-HTTP/2
gate.

This correction does not change:

- compression registration or configured enablement;
- rejection of native gRPC on a frontend without HTTP/2;
- GRPCRoute/HTTPRoute selection or upstream HTTP/2 selection;
- gRPC-Web trailer conversion, request/response streaming, or body buffering;
- access logs, diagnostics, metrics, or tracing.

## Verification

Inline tests in `pg_init_downstream_modules.rs` build typed module contexts with
compression level zero and the default enabled level. Both contain
`GrpcWebBridge` and `ResponseCompression`; compression remains disabled/enabled
according to its configured level. Live H1 session tests in
`pg_early_request_filter.rs` run the real Pingora module header filter and pin
three discriminating cases: gRPC-Web survives the header rewrite and bypasses
the native-gRPC HTTP/2 gate; native gRPC still triggers that gate; and an exact
WebSocket upgrade keeps the bridge disabled and the gRPC-Web Content-Type
unchanged. This is module/filter sequencing coverage, not a claim of HTTP/2
frame-level or end-to-end integration coverage.

## Re-evaluation triggers

Re-evaluate if Pingora removes its `GrpcWeb` module, changes bridge activation
away from per-request context initialization, or introduces a separate explicit
operator policy for gRPC-Web availability.

## Source references

- `edgion-gateway/src/routes/http/proxy_http/pg_init_downstream_modules.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_early_request_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
- Pingora `pingora-core/src/modules/http/grpc_web.rs`
- Pingora `pingora-proxy/src/lib.rs` request-filter ordering
