# Edgion Integration Test Configurations

This directory contains the Kubernetes Gateway API manifests used by the integration test suites.

## Directory Layout

```text
conf/
├── base/                   # Shared base configuration for all suites
│   ├── GatewayClass.yaml
│   ├── Gateway.yaml
│   ├── EdgionGatewayConfig.yaml
│   ├── EdgionTls_edge_edge-tls.yaml
│   └── Secret_edgion-test_edge-tls.yaml
├── http/                   # Basic HTTP tests
├── grpc/                   # Basic gRPC tests
├── tcp/                    # TCP tests
├── udp/                    # UDP tests
├── http-match/             # HTTP matching tests
├── grpc-match/             # gRPC matching tests
├── grpc-tls/               # gRPC over TLS tests
├── lb-roundrobin/          # RoundRobin load-balancing tests
├── lb-consistenthash/      # ConsistentHash load-balancing tests
├── weighted-backend/       # Weighted backend tests
├── timeout/                # Timeout tests
├── plugins/                # Plugin tests
├── redirect/               # HTTP redirect tests
├── stream-plugins/         # Stream plugin tests
├── mtls/                   # Mutual TLS tests
├── backend-tls/            # Backend TLS tests
└── EdgionTls/
    └── cipher/             # TLS cipher suite tests
```

## Load Order

1. `base/` must be loaded first because it contains shared resources such as `GatewayClass` and `Gateway`.
2. Load individual suite directories as needed.

## Usage

```bash
# Start the full local test stack and load configuration
./examples/test/scripts/utils/start_all_with_conf.sh

# Start the stack and load only selected suites
./examples/test/scripts/utils/start_all_with_conf.sh --suites http,grpc

# Load a single suite after the controller is already running
./examples/test/scripts/utils/load_conf.sh http

# Load every suite
./examples/test/scripts/utils/load_conf.sh all
```

## Configuration Reference

### `base/`

- `GatewayClass.yaml`: Defines the Gateway class and links it to `EdgionGatewayConfig`.
- `Gateway.yaml`: Defines listeners (HTTP:10080, HTTPS:10443, TCP:19000, UDP:19002, gRPC:18443).
- `EdgionGatewayConfig.yaml`: Edgion-specific settings such as timeouts, Real IP, and security options.
- `EdgionTls_edge_edge-tls.yaml`: TLS configuration that references a Secret.
- `Secret_edgion-test_edge-tls.yaml`: TLS certificate Secret.

### `http/`

- `Service_test-http.yaml`: HTTP test service.
- `EndpointSlice_test-http.yaml`: Backend discovery for `127.0.0.1:30001`.
- `HTTPRoute.yaml`: HTTP routing rules for `test.example.com`.
- `Service_test-websocket.yaml`: WebSocket service.
- `EndpointSlice_test-websocket.yaml`: WebSocket backend discovery.

### `http-match/`

- `HTTPRoute_default_match-test.yaml`: HTTP route matching tests.
- `HTTPRoute_section-test.yaml`: `sectionName` matching tests.
- `HTTPRoute_wildcard.yaml`: Wildcard hostname tests.

### `grpc/`

- `Service_test-grpc.yaml`: gRPC test service.
- `EndpointSlice_test-grpc.yaml`: gRPC backend discovery.
- `GRPCRoute.yaml`: gRPC routing rules.

### `grpc-match/`

- `GRPCRoute_edge_match-test.yaml`: gRPC matching tests.
- `GRPCRoute_edge_match-test-wrong-section.yaml`: Invalid `sectionName` test.

### `grpc-tls/`

- `GRPCRoute_edge_test-grpc-https.yaml`: gRPC over TLS route.

### `lb-roundrobin/`

RoundRobin load-balancing tests verify that requests are evenly distributed across backends.

- `Gateway.yaml`: RoundRobin test gateway on port `31120`.
- `Service_default_lb-rr.yaml`: Load-balancing service definition.
- `EndpointSlice_default_lb-rr.yaml`: Single-slice backend with 3 endpoints.
- `Endpoints_default_lb-rr.yaml`: `Endpoints` resource for EP mode tests.
- `HTTPRoute_default_lb-rr-eps.yaml`: EndpointSlice mode route.
- `HTTPRoute_default_lb-rr-ep.yaml`: Endpoints mode route using `kind: ServiceEndpoint`.
- `Service_default_lb-rr-multi.yaml`: Multi-slice service.
- `EndpointSlice_default_lb-rr-multi-1/2.yaml`: Multi-slice backend with 4 endpoints across 2 slices.
- `HTTPRoute_default_lb-rr-multi.yaml`: Multi-slice route.

Test scenarios:
1. EndpointSlice mode with a single slice.
2. Endpoints mode using `ServiceEndpoint`.
3. Round-robin behavior across multiple EndpointSlices.

### `lb-consistenthash/`

ConsistentHash tests verify that the same key is consistently routed to the same backend.

- `Gateway.yaml`: ConsistentHash test gateway on port `31121`.
- `Service_default_lb-ch.yaml`: Service definition.
- `EndpointSlice_default_lb-ch.yaml`: Single-slice backend.
- `Endpoints_default_lb-ch.yaml`: `Endpoints` resource.
- `HTTPRoute_default_lb-ch-header-eps.yaml`: Header hash with EndpointSlice mode.
- `HTTPRoute_default_lb-ch-header-ep.yaml`: Header hash with Endpoints mode.
- `HTTPRoute_default_lb-ch-cookie.yaml`: Cookie hash route.
- `HTTPRoute_default_lb-ch-arg.yaml`: Query-parameter hash route.
- `Service_default_lb-ch-multi.yaml`: Multi-slice test service.
- `EndpointSlice_default_lb-ch-multi-1/2.yaml`: Multi-slice backend.
- `HTTPRoute_default_lb-ch-multi.yaml`: Multi-slice consistent-hash route.

Test scenarios:
1. Header hash using `x-user-id`.
2. Cookie hash using `session-id`.
3. Query-parameter hash using `user_id`.
4. EndpointSlice versus Endpoints mode.
5. Consistency across multiple slices.

### `weighted-backend/`

- `Service_edge_backend-*.yaml`: Weighted backend services.
- `EndpointSlice_edge_backend-*.yaml`: Backend discovery for multiple services.
- `HTTPRoute_default_weighted-backend.yaml`: Weighted routing rules.

### `timeout/`

- `EdgionPlugins_default_timeout-debug.yaml`: Timeout debugging plugin.
- `HTTPRoute_default_timeout-backend.yaml`: Timeout test route.

### `mtls/`

- `Gateway_edge_mtls-test-gateway.yaml`: mTLS test gateway.
- `EdgionTls_edge_mtls-test-*.yaml`: Various mTLS configurations.
- `HTTPRoute_edge_mtls-test.yaml`: mTLS test route.

### `EdgionTls/cipher/`

These tests verify that `EdgionTls` supports custom TLS 1.2 cipher lists.

- `Gateway.yaml`: Cipher test gateway on ports `31195/31196`.
- `HTTPRoute.yaml`: Cipher test route.
- `EdgionTls_cipher_legacy.yaml`: TLS 1.2 with legacy ciphers such as `AES128-SHA`.
- `EdgionTls_cipher_modern.yaml`: TLS 1.2 with modern ciphers such as `ECDHE-RSA-AES256-GCM-SHA384`.

The test cases use `openssl s_client` to verify the negotiated server cipher.

### `backend-tls/`

- `BackendTLSPolicy_edge_backend-tls.yaml`: Backend TLS policy.
- `Service_edge_test-backend-tls.yaml`: Backend TLS service.
- `EndpointSlice_edge_test-backend-tls.yaml`: Backend TLS discovery.
- `HTTPRoute_edge_backend-tls.yaml`: Backend TLS route.
- `Secret_backend-ca.yaml`: Backend CA certificate.

### `ref-grant-status/`

ReferenceGrant and status integration tests verify status updates for cross-namespace references.

- `Service_backend_cross-ns-svc.yaml`: Service in the `backend` namespace.
- `EndpointSlice_backend_cross-ns-svc.yaml`: Backend service discovery.
- `HTTPRoute_app_cross-ns-route.yaml`: `HTTPRoute` in `edgion-default` that references a Service in `edgion-backend`.
- `HTTPRoute_app_cross-ns-denied.yaml`: `HTTPRoute` in `edgion-default` that references a Service in `edgion-system` without a `ReferenceGrant`.
- `HTTPRoute_app_multi-parent.yaml`: Multiple `parentRefs` test in `edgion-default`.
- `ReferenceGrant_backend_allow-app.yaml`: Allows `HTTPRoute` in `edgion-default` to reference the Service in `edgion-backend`.

Note: the `app` segment in filenames is historical and the current test setup no longer creates additional `app` or `other` namespaces.

Test scenarios:
1. Cross-namespace reference with a `ReferenceGrant` results in `ResolvedRefs=True`.
2. Cross-namespace reference without a `ReferenceGrant` results in `ResolvedRefs=False (RefNotPermitted)`.
3. A late `ReferenceGrant` automatically requeues the `HTTPRoute` and updates status.
4. Each `parentRef` gets an independent status entry.

## Adding a New Suite

1. Create the suite directory.
2. Add the required resources such as `Service`, `EndpointSlice`, and `Route`.
3. The loader will discover and load the suite automatically based on the directory name.
