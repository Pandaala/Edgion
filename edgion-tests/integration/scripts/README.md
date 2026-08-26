# Edgion Test Scripts

This directory contains the test and build scripts used by the Edgion project.

## Directory Layout

```text
scripts/
├── gen_certs/          # Runtime certificate generation scripts
│   ├── generate_tls_certs.sh
│   ├── generate_backend_certs.sh
│   ├── generate_mtls_certs.sh
│   └── generate_conf_sync_certs.sh   # conf_sync gRPC mTLS certs (uses edgion-cli gen-certs)
├── ci/                 # CI/CD scripts
│   └── check.sh        # fmt/clippy/unit test checks
├── conf-sync/          # conf_sync channel tests
│   ├── run_conf_sync_test.sh          # Leak detection test
│   ├── run_conf_sync_advanced_test.sh # Advanced leak detection
│   └── run_conf_sync_mtls_test.sh     # mTLS end-to-end test
├── integration/        # Integration test scripts
│   ├── run_direct.sh   # Direct tests without the Gateway
│   └── run_integration.sh  # End-to-end tests through the Gateway
└── utils/              # Utility scripts
    ├── prepare.sh      # Prebuild test components (build-only, no runtime artifacts)
    ├── conf_sync_certs.sh      # Shared conf_sync mTLS helpers (see below)
    ├── start_all.sh    # Start all test services
    ├── start_all_with_conf.sh  # Start with config loading
    └── kill_all.sh     # Stop all test services
```

## conf_sync transport: mTLS everywhere

`config/edgion-{controller,gateway,controller-etcd}.yaml` ship the **mTLS**
`conf_sync_security` block; nothing in the default path rewrites it. The cert paths are
work-dir relative (`certs/grpc/primary/...`), so a runner only has to place the cert set
in the work dir of the process it starts:

```bash
source utils/conf_sync_certs.sh
conf_sync_distribute_certs "$WORK_DIR"   # generate (idempotent) directly into $WORK_DIR/certs/grpc/
```

A script that generates its **own** binary config must never hardcode a transport — it
appends the shared emitter's output instead:

```bash
conf_sync_security_block server >> controller.yaml   # cert/key/ca for the Controller
conf_sync_security_block client >> gateway.yaml      # cert/key/ca for the Gateway
```

Every cert set is owned by the per-run work directory of the process it serves —
no script writes certificates into the repo root or any shared cache. `prepare.sh`
is build-only. A developer who intentionally wants a bare `cargo run` off the
checked-in configs (work dir = repo root) bootstraps the cert set explicitly:

```bash
cargo run --bin edgion-cli -- gen-certs --output file --output-dir certs/grpc --cert-name primary
```

`skip_tls: true` (plaintext) survives in exactly one place: `run_conf_sync_skip_tls_metric_test.sh`,
where it is the subject under test (driven through `start_all_with_conf.sh`'s
`EDGION_CONF_SYNC_PLAINTEXT=1` toggle).

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

# Run one resource family or one exact item
./scripts/integration/run_integration.sh -r HTTPRoute
./scripts/integration/run_integration.sh -r HTTPRoute -i Match

# Reuse already prepared binaries, include slow tests, or load explicit fixture suites
./scripts/integration/run_integration.sh --no-prepare -r HTTPRoute -i Basic
./scripts/integration/run_integration.sh --full-test
./scripts/integration/run_integration.sh --suites base,HTTPRoute/Basic -r HTTPRoute -i Basic
```

Run `./scripts/integration/run_integration.sh --help` for the complete current option list. There is
no implicit exclusion flag: narrow coverage explicitly with `--resource` and `--item`. The canonical
integration architecture and workflow are documented in
[`skills/05-testing/01-integration-testing.md`](../../../skills/05-testing/01-integration-testing.md).

Covered areas:
- Basic protocols: `http`, `https`, `grpc`, `grpc-tls`, `websocket`, `tcp`, `udp`
- Route matching: `http-match`, `grpc-match`
- HTTP features: `http-redirect`, `http-security`
- TLS: `mtls`, `backend-tls`
- Load balancing: `lb-rr` (RoundRobin), `lb-ch` (ConsistentHash), `weighted-backend`
- Advanced features: `timeout`, `real-ip`, `security`, `plugin-logs`

## conf_sync Tests

### `run_conf_sync_mtls_test.sh`

End-to-end test for the conf_sync gRPC mTLS feature. Verifies that Controller and
Gateway can communicate over a mutually authenticated TLS channel.

```bash
# Full run: generate certs → start with mTLS → verify → stop
./scripts/conf-sync/run_conf_sync_mtls_test.sh

# Attach to a running mTLS environment
./scripts/conf-sync/run_conf_sync_mtls_test.sh --no-start

# Keep services running after test
./scripts/conf-sync/run_conf_sync_mtls_test.sh --keep-alive
```

The test performs the following verifications:
- mTLS log entries confirm TLS is active on both sides
- Controller and Gateway pass health checks
- Gateway reaches `ready` state (implies successful mTLS-based config sync)
- `server_id` matches between Controller and Gateway
- Gateway has synced resources from Controller
- `resource_diff` passes over the mTLS channel
- Plain-text connections to the mTLS port are rejected

### `run_conf_sync_test.sh` / `run_conf_sync_advanced_test.sh`

Leak detection tests for the conf_sync pipeline (inject/delete cycles).

## Utility Scripts

### `prepare.sh`

Prebuilds all binaries required by the test environment in debug mode.

```bash
# Build all components
./scripts/utils/prepare.sh
```

Built artifacts under `target/debug/`:
- `edgion-controller`: configuration controller
- `edgion-gateway`: gateway service
- `edgion-cli`: command-line tool
- `test_server`: backend test server
- `test_client`: integration test client
- `test_client_direct`: direct test client
- `resource_diff`: Controller/Gateway synchronization validator
- `config_load_validator`: configuration-load validator
- `cache_residue`: final cache-residue validator
- `credential_reservation_redis`: stable copy of the precompiled ignored Redis integration test

Output location:

```text
target/debug/
├── edgion-controller
├── edgion-gateway
├── edgion-cli
├── test_server
├── test_client
├── test_client_direct
├── resource_diff
├── config_load_validator
├── cache_residue
└── credential_reservation_redis
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
