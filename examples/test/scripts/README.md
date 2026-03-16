# Edgion Test Scripts

This directory contains the test and build scripts used by the Edgion project.

## Directory Layout

```text
scripts/
├── gen_certs/          # Runtime certificate generation scripts
├── ci/                 # CI/CD scripts
│   └── check.sh        # fmt/clippy/unit test checks
├── integration/        # Integration test scripts
│   ├── run_direct.sh   # Direct tests without the Gateway
│   └── run_integration.sh  # End-to-end tests through the Gateway
└── utils/              # Utility scripts
    ├── prepare.sh      # Prebuild test components
    ├── start_all.sh    # Start all test services
    └── kill_all.sh     # Stop all test services
```

## CI Scripts

### `check.sh`

Runs code quality checks such as formatting, linting, and unit tests.

```bash
# Run all checks
./scripts/ci/check.sh

# Check formatting only
./scripts/ci/check.sh -f

# Run clippy only
./scripts/ci/check.sh -c

# Run unit tests only
./scripts/ci/check.sh -t

# Auto-fix issues
./scripts/ci/check.sh --fix

# Show verbose output
./scripts/ci/check.sh -v
```

## Integration Test Scripts

### `run_direct.sh`

Tests connectivity between `test_client` and `test_server` directly, without passing through the Gateway.

```bash
./scripts/integration/run_direct.sh
```

Covered protocols:
- `http`
- `grpc`
- `websocket`
- `tcp`
- `udp`

### `run_integration.sh`

Runs full end-to-end integration tests through the Gateway.

```bash
# Run all integration tests
./scripts/integration/run_integration.sh

# Run a specific test
./scripts/integration/run_integration.sh --test http-match

# Skip selected tests
./scripts/integration/run_integration.sh --skip "mtls,backend-tls"
```

Covered areas:
- Basic protocols: `http`, `https`, `grpc`, `grpc-tls`, `websocket`, `tcp`, `udp`
- Route matching: `http-match`, `grpc-match`
- HTTP features: `http-redirect`, `http-security`
- TLS: `mtls`, `backend-tls`
- Load balancing: `lb-rr` (RoundRobin), `lb-ch` (ConsistentHash), `weighted-backend`
- Advanced features: `timeout`, `real-ip`, `security`, `plugin-logs`

## Utility Scripts

### `prepare.sh`

Prebuilds all binaries required by the test environment in debug mode.

```bash
# Build all components
./scripts/utils/prepare.sh
```

Built artifacts:
- `edgion-controller`: configuration controller
- `edgion-gateway`: gateway service
- `edgion-ctl`: command-line tool
- `test_server`: backend test server
- `test_client`: integration test client
- `test_client_direct`: direct test client

Output location:

```text
target/debug/
├── edgion-controller
├── edgion-gateway
├── edgion-ctl
└── examples/
    ├── test_server
    └── test_client
```

### `start_all.sh`

Starts all local test services, including `test_server`, `controller`, and `gateway`.

```bash
# Start all services
./scripts/utils/start_all.sh
```

### `kill_all.sh`

Stops all Edgion-related processes.

```bash
# Stop all services
./scripts/utils/kill_all.sh
```
