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

### HTTP/1-only integration coverage

The follow-up `add-grpc-web-http1-only-gateway-integration` extends the existing
`HTTPRoute/Protocol/GrpcWeb` selection with a second suite on port 31083, resolved
through its own `ports.json` key. Its Gateway sets
`edgion.io/enable-http2: "false"`; the original port 31081 listener remains
H2-enabled. Both HTTPRoutes reach the dedicated `test-grpcweb` Service using
`appProtocol: kubernetes.io/h2c`. Matching Gateway/HTTPRoute fixtures are present
in the local and Kubernetes configuration trees.
The Kubernetes Gateway Service also exposes TCP 31083, because the test client
connects through that Service rather than directly to the Gateway Pod.

Both suites force HTTP/1.1 and reuse the same gRPC-Web framing checks. The unary
success case requires HTTP 200 over HTTP/1.1, gRPC-Web Content-Type, no
`grpc-status` response header, exactly one uncompressed data frame followed by
one `0x80` trailer frame, a decoded `HelloResponse.message` greeting for the sent
name, and an exact successful `grpc-status` trailer value. The existing
unrouted-path trailer case also runs on both listeners. Requests have a
10-second timeout, within the framework's 30-second case deadline. No process
or resource is created by the suite; the standard runner owns fixture loading
and topology cleanup.

Run the paired suites with:

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r HTTPRoute -i Protocol/GrpcWeb
```

On 2026-09-09 the command above passed all four cases, with zero infrastructure
errors. The Gateway startup log confirmed that HTTP/2 was disabled for
`edgion-test/grpcweb-http1-only-gateway` and that port 31083 used plain HTTP.
The run report (`tmp/integration_testing/testing_20260909_095823/report.log`)
recorded 17 passed gates/selections and one skipped build; final metrics-label,
unknown-field, requeue-loop, and cache-residue checks passed. Standard cleanup
stopped all three test processes. The first attempt stopped before test execution
because `cache_residue` was absent; rebuilding the required binaries resolved it.

`cargo fmt --all`, `cargo check --workspace --all-targets`,
`cargo clippy --workspace --all-targets`, agent-doc validation, SSA-force,
metrics-inventory, and Pingora-source guards passed. Clippy reported warnings in
existing code; none points into the changed gRPC-Web test code. Local/Kubernetes
fixture equality, unique port mapping, and focused Kubernetes fixture ownership
checks passed. The Gateway Service exposes the new port to the Kubernetes test
client. A live Kubernetes run was not performed.

The extra repository-wide `validate_suite_parity.sh` check failed on unrelated
pre-existing missing entries, reproduced with the unmodified HEAD test-client
manifest. The inventory follow-up (ITEST-K8S-INVENTORY-PARITY) was removed at
the user's request; any future reconciliation is left to the user. Standard
integration and Kubernetes-specific tests are separate suites, so this inventory
difference alone does not establish a required coverage gap or a completed fix.
These unary binary-mode cases do not establish grpc-web-text, server-streaming,
or independent H2 frame-level coverage.

Resolution: `add-grpc-web-http1-only-gateway-integration` (B0183 follow-up), fixed
by extending the existing suite after separate user authorization to add tests.
The runtime correction and its historical module-level tests remain unchanged.

### Shared-suite backend identity (P090-002)

The 2026-10-05 combined local acceptance run exposed a fixture collision:
GrpcWeb and GRPCRoute/Basic both declared `edgion-test/test-grpc`, but only
GrpcWeb supplied `appProtocol: kubernetes.io/h2c`. Loading GRPCRoute/Basic
last removed that protocol hint. Both gRPC-Web success cases then reached
the H2-only origin using HTTP/1, producing upstream `InvalidHTTPHeader` and
a body-encoded INTERNAL status instead of a protobuf reply.

GrpcWeb now owns `test-grpcweb`, including its local EndpointSlice and both
HTTPRoute backendRefs. Kubernetes uses the same Service identity with the
existing `app: edgion-test-server` selector, allowing Kubernetes to generate
the endpoints. GRPCRoute/Basic retains its original hint-free Service, so
native-gRPC protocol inference coverage is not weakened. Keep these resource
identities distinct when combining suites; a green standalone run cannot
establish that the merged fixtures preserve the protocol hint.

The corrected fixtures were loaded together, with GRPCRoute/Basic after
GrpcWeb, in the standard shared topology
`tmp/integration_testing/testing_20261005_210018`. All four gRPC-Web cases,
three GRPCRoute/Basic cases, and 32 BodyPlugins cases passed, followed by the
final residue check. The test client received the actual Gateway binary's
`--print-build-capabilities` output for the BodyPlugins capability gate.
Workspace checks, both suite-parity scripts, and static Kubernetes selector /
backend-reference checks passed. No live Kubernetes run was performed.

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
- `edgion-tests/src/client/suites/grpc_route/grpc_web.rs`
- `edgion-tests/integration/conf/HTTPRoute/Protocol/GrpcWeb/`
- `edgion-tests/k8s/conf/HTTPRoute/Protocol/GrpcWeb/`
- `edgion-tests/k8s/kubernetes/gateway/service.yaml`
