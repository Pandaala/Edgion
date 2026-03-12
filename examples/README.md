# Edgion Examples & Tests

This directory contains integration tests, conformance tests, and example configurations for the Edgion Gateway.

## Directory Structure

```
examples/
├── code/                          # Rust integration test code
│   ├── client/                    # Test client framework & suites
│   │   └── suites/                # Per-feature test suites
│   ├── server/                    # Test backend servers
│   └── validator/                 # Configuration validators
├── gateway-api-conformance/       # Gateway API conformance tests (Go)
│   ├── conformance_test.go
│   └── manifests/
└── k8stest/                       # Kubernetes integration tests
    ├── conf/                      # Test YAML manifests
    ├── kubernetes/                # Deployment manifests
    └── scripts/                   # Test runner scripts
```

## Integration Tests

### Running Unit Tests

```bash
cargo test --all --tests
```

### Running Local Integration Tests

```bash
cd examples/testing
./run_integration_test.sh
```

### Running Kubernetes Integration Tests

See [k8stest/README.md](k8stest/README.md) for detailed instructions.

```bash
# Full Kubernetes integration test
./examples/k8stest/scripts/run_k8s_integration.sh

# Run specific test suite
./examples/k8stest/scripts/run_k8s_integration.sh -r HTTPRoute -i Match

# Run specific plugin tests
./examples/k8stest/scripts/run_k8s_integration.sh --start-from EdgionPlugins/JwtAuth
```

### Gateway API Conformance Tests

```bash
cd examples/gateway-api-conformance
go test -v -run TestConformance
```

## Test Configuration

Test YAML manifests are organized under `k8stest/conf/` by resource type:

- `EdgionPlugins/` — Plugin test configurations (BasicAuth, CORS, JWT, RateLimit, etc.)
- `HTTPRoute/` — HTTP routing test configurations
- `GRPCRoute/` — gRPC routing tests
- `TCPRoute/` — TCP routing tests
- `UDPRoute/` — UDP routing tests
- `TLSRoute/` — TLS routing tests
- `Gateway/` — Gateway configuration tests

## Related Documentation

- [User Guide](../docs/en/user-guide/README.md)
- [Ops Guide](../docs/en/ops-guide/README.md)
- [Developer Guide](../docs/en/dev-guide/README.md)
