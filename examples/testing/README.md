# Edgion Testing Framework Documentation

This document provides comprehensive information about Edgion's testing infrastructure, including automated integration tests, test utilities, and configuration management.

## Table of Contents

- [Overview](#overview)
- [Testing Architecture](#testing-architecture)
- [Integration Test Pipeline](#integration-test-pipeline)
- [Quick Start](#quick-start)
- [Manual Service Operations](#manual-service-operations)
- [Test Client CLI](#test-client-cli)
- [Manual Testing Commands](#manual-testing-commands)
- [Port Reference](#port-reference)
- [TLS Certificates](#tls-certificates)
- [Log Files](#log-files)
- [Configuration Files](#configuration-files)
- [Troubleshooting](#troubleshooting)
- [For AI Assistants](#for-ai-assistants)

---

## Overview

The Edgion testing framework provides:
- **Unified test infrastructure**: Single test server and client supporting all protocols (HTTP, HTTPS, gRPC, TCP, UDP, WebSocket)
- **Automated integration tests**: End-to-end validation of all gateway features
- **Configuration validation**: Ensures all YAML configs are correctly loaded
- **Resource synchronization checks**: Verifies controller-gateway state consistency
- **Comprehensive test reports**: Detailed pass/fail reporting with timing information

---

## Testing Architecture

### Components

```
┌─────────────────────────────────────────────────────────────┐
│                    Integration Test Pipeline                 │
│  (run_integration_test.sh)                                   │
└─────────────────────────────────────────────────────────────┘
                           │
                           ├──────────────────────────┐
                           ▼                          ▼
              ┌────────────────────┐      ┌──────────────────┐
              │  Configuration     │      │  Resource        │
              │  Load Validator    │      │  Synchronization │
              │  (Step 3.5)        │      │  Validator       │
              │                    │      │  (Step 4.5)      │
              └────────────────────┘      └──────────────────┘
                           │                          │
                           ▼                          ▼
              ┌─────────────────────────────────────────────┐
              │         edgion-controller                    │
              │  - Admin API (8080)                         │
              │  - gRPC API (50051)                         │
              │  - Auto-loads examples/conf/*.yaml          │
              └─────────────────────────────────────────────┘
                           │
                           ▼
              ┌─────────────────────────────────────────────┐
              │         edgion-gateway                       │
              │  - HTTP (10080), HTTPS (10443)              │
              │  - gRPC (10080, 18443)                      │
              │  - TCP (19000, 19001), UDP (19002)          │
              └─────────────────────────────────────────────┘
                           │
                           ▼
              ┌─────────────────────────────────────────────┐
              │         test_server                          │
              │  - HTTP (30001-30004)                       │
              │  - gRPC (30021)                             │
              │  - TCP (30010), UDP (30011)                 │
              │  - WebSocket (30005)                        │
              └─────────────────────────────────────────────┘
                           │
                           ▼
              ┌─────────────────────────────────────────────┐
              │         test_client                          │
              │  - Protocol-specific test suites            │
              │  - Direct & Gateway modes                   │
              │  - Detailed validation & reporting          │
              └─────────────────────────────────────────────┘
```

### Test Modes

1. **Direct Mode**: Tests backend services directly (no gateway)
   - Validates `test_server` functionality
   - Quick smoke tests for protocol implementations
   - Baseline for gateway comparison

2. **Gateway Mode**: Tests through edgion-gateway
   - End-to-end validation of routing, load balancing, TLS, plugins
   - Requires `edgion-controller` and `edgion-gateway` running
   - Uses Gateway API resources (HTTPRoute, GRPCRoute, etc.)

---

## Integration Test Pipeline

The `run_integration_test.sh` script executes a comprehensive test pipeline with multiple stages:

### Pipeline Stages

#### 1. Pre-flight Checks
- Validates script is run from correct directory
- Checks for stale processes from previous runs
- Creates log directory structure

#### 2. Service Startup
- **test_server**: Starts backend services for all protocols
- **edgion-controller**: Starts controller with auto-config loading from `examples/conf/`
- **edgion-gateway**: Starts gateway and connects to controller

#### 3. Configuration Validation (Step 3.5)

**Tool**: `cargo run --example config_load_validator`

**Purpose**: Ensures all YAML configuration files are successfully loaded by the controller

**Process**:
1. Scans `examples/conf/` for all `*.yaml` files
2. Parses metadata from each file: `kind`, `name`, `namespace`, `annotations`
3. Queries Controller Admin API to verify resource exists:
   - Namespaced resources: `GET /api/v1/namespaced/{kind}?namespace={ns}&name={name}`
   - Cluster-scoped: `GET /api/v1/cluster/{kind}?name={name}`
4. Auto-skips:
   - Multi-document YAML files (e.g., `BackendTLSPolicy_example.yaml`)
   - Base configuration resources (GatewayClass, EdgionGatewayConfig, Gateway)
   - Resources with annotation: `edgion.io/skip-load-validation: "true"`
5. Reports:
   - ✅ Loaded resources (green)
   - ⏭️  Skipped resources with reasons (yellow)
   - ❌ Failed to load resources (red)
6. Exit code: 0 if all non-skipped resources loaded, 1 otherwise

**Why This Matters**:
- Catches YAML syntax errors early (e.g., incorrect field names, missing required fields)
- Prevents cascading test failures due to missing configurations
- Provides clear diagnostic output for configuration issues
- Saves debugging time by failing fast before functional tests

**Example Output**:
```
Configuration Load Validation Results:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

✅ Loaded (45):
  - HTTPRoute default/example-route
  - Service edge/test-http
  - EdgionPlugins default/debug-access-log-test
  ...

⏭️  Skipped (8):
  - Gateway edge/example-gateway (base_conf resource)
  - EdgionPlugins default/test-skip-validation (annotation: skip-load-validation)
  - BackendTLSPolicy example (multi-document YAML)
  ...

Total: 53 | Loaded: 45 | Skipped: 8 | Failed: 0
```

#### 4. Resource Synchronization Check (Step 4.5)

**Tool**: `cargo run --example resource_diff`

**Purpose**: Verifies controller and gateway have consistent resource states

**Process**:
1. Queries Controller Admin API: `GET /api/v1/all_resources`
2. Queries Gateway Admin API: `GET /api/v1/all_resources`
3. Compares resource counts and details across:
   - HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute
   - Service, EndpointSlice
   - Gateway, GatewayClass
   - EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, EdgionTls
   - Secret, ConfigMap, ReferenceGrant, PluginMetaData, LinkSys
4. Reports differences if any

**Why This Matters**:
- Ensures controller successfully pushed all resources to gateway
- Catches gRPC communication issues between controller and gateway
- Validates resource reconciliation logic
- Critical prerequisite for functional tests (gateway needs all routes/services)

**Example Output**:
```
Resource Synchronization Check:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

HTTPRoute:        ✅ 15 (controller) == 15 (gateway)
Service:          ✅ 12 (controller) == 12 (gateway)
EdgionPlugins:    ✅ 8 (controller) == 8 (gateway)
...

✨ All resources synchronized!
```

#### 5. Functional Test Execution

**Test Suites** (in order):
1. **Direct Mode Tests**: Validates test_server directly
   - HTTP basic, gRPC, TCP, UDP, WebSocket
2. **Gateway Mode Tests**: Full gateway functionality
   - HTTP routing, gRPC routing, TCP/UDP proxying, WebSocket
3. **HTTPS Tests**: TLS termination and routing
4. **gRPC-TLS Tests**: gRPC over TLS (18443)
5. **mTLS Tests**: Mutual TLS authentication
   - Server validation, client validation, certificate chain, SAN validation
6. **Plugin Logs Tests**: Plugin execution and logging
   - Request filter plugins (CORS, CSRF, etc.)
   - Response filter plugins (ResponseHeaderModifier, DebugAccessLogToHeader)
   - Verifies plugin log structure and execution order

**Test Client Framework**:
- Each suite contains multiple test cases
- Each test case validates specific functionality with assertions
- Detailed pass/fail reporting with timing
- Logs failures with context for debugging

#### 6. Service Cleanup
- Gracefully terminates all services (test_server, gateway, controller)
- Ensures clean state for next run

#### 7. Unified Test Report

**Format**:
```
==========================================
  Unified Test Report
==========================================

Test Run: 2025-12-27 15:30:45
Total Tests: 7
Passed: 7
Failed: 0

Test Details:
✅ Direct Mode                          PASSED
✅ Gateway Mode                         PASSED
✅ HTTPS Gateway Mode                   PASSED
✅ gRPC-TLS Gateway Mode               PASSED
✅ mTLS Gateway Mode                    PASSED
✅ Plugin Logs Gateway Mode             PASSED
✅ Resource Synchronization             PASSED

Overall Result: PASSED ✨
```

**Benefits**:
- Quick at-a-glance validation that all tests passed
- Easy to scan for failures when troubleshooting
- Provides timestamps and test counts for tracking
- Simplifies CI/CD integration

### Running the Full Pipeline

```bash
cd examples/testing
./run_integration_test.sh
```

**Expected Duration**: ~60-90 seconds (varies by system)

**Success Criteria**:
- All configuration files loaded ✅
- Resources synchronized between controller and gateway ✅
- All 7 test suites pass ✅
- No errors in service logs ✅

---

## Quick Start

### One-Command Integration Test (Recommended)

```bash
cd examples/testing
./run_integration_test.sh
```

**What it does**:
1. Generates TLS certificates (if needed)
2. Starts test_server (backend services)
3. Starts edgion-controller (loads `examples/conf/` configs)
4. Validates all configurations are loaded
5. Starts edgion-gateway
6. Verifies resource synchronization
7. Runs all test suites (Direct + Gateway modes, including HTTPS, gRPC-TLS, mTLS, Plugins)
8. Displays unified test report
9. Cleans up services

**Log files** are saved to `examples/testing/logs/`

---

## Manual Service Operations

### 1. Start Backend Test Server

```bash
cargo run --example test_server
```

**Services started**:
- HTTP: `30001-30004` (4 instances for load balancing tests)
- gRPC: `30021`
- WebSocket: `30005`
- TCP: `30010`
- UDP: `30011`

### 2. Start Controller

```bash
cargo run --bin edgion-controller
```

**Default configuration**:
- gRPC API: `50051` (for gateway connection)
- Admin API: `8080` (for health checks and resource queries)
- Config directory: `examples/conf` (auto-loaded on startup)
- GatewayClass: `public-gateway`

**Admin API Endpoints**:
- `GET /health` - Health check
- `GET /api/v1/all_resources` - List all loaded resources (summary)
- `GET /api/v1/namespaced/{kind}?namespace={ns}&name={name}` - Query namespaced resource
- `GET /api/v1/cluster/{kind}?name={name}` - Query cluster-scoped resource

### 3. Start Gateway

```bash
cargo run --bin edgion-gateway
```

**Listeners started** (based on Gateway resources):
- HTTP: `10080`
- HTTPS: `10443` (requires TLS certificates)
- gRPC (HTTP): `10080` (HTTP/2 cleartext)
- gRPC (HTTPS): `18443` (HTTP/2 over TLS)
- TCP: `19000`
- TCP with TLS Terminate: `19001`
- UDP: `19002`

**Admin API Endpoints**:
- `GET /health` - Health check
- `GET /api/v1/all_resources` - List all received resources from controller

---

## Test Client CLI

### Basic Usage

```bash
# Syntax
cargo run --example test_client -- [OPTIONS] <COMMAND>

# Options
  -g, --gateway          # Enable Gateway mode (default: Direct mode)
  -v, --verbose          # Verbose output
  --json                 # JSON format report
  --http-port <PORT>     # Custom HTTP port (Direct mode, default: 30001)
  --grpc-port <PORT>     # Custom gRPC port (Direct mode, default: 30021)
  --tcp-port <PORT>      # Custom TCP port (Direct mode, default: 30010)
  --udp-port <PORT>      # Custom UDP port (Direct mode, default: 30011)
  --websocket-port <PORT> # Custom WebSocket port (Direct mode, default: 30005)
  --https-port <PORT>    # Custom HTTPS port (Gateway mode, default: 10443)

# Commands
  http          # HTTP tests
  grpc          # gRPC tests
  grpc-tls      # gRPC-TLS tests (Gateway mode only)
  tcp           # TCP tests
  udp           # UDP tests
  websocket     # WebSocket tests
  https         # HTTPS tests (Gateway mode only)
  tls-tcp       # TLS Terminate to TCP tests (Gateway mode only)
  mtls          # mTLS tests (Gateway mode only)
  plugin-logs   # Plugin logging tests (Gateway mode only)
  all           # Run all tests
```

### Direct Mode Examples

```bash
# Test all protocols (direct connection to backend)
cargo run --example test_client -- all

# Test individual protocols
cargo run --example test_client -- http
cargo run --example test_client -- grpc
cargo run --example test_client -- tcp
cargo run --example test_client -- udp
cargo run --example test_client -- websocket

# Verbose output
cargo run --example test_client -- --verbose http

# Custom port
cargo run --example test_client -- --http-port 8080 http
```

### Gateway Mode Examples

```bash
# Test all protocols (through Gateway)
cargo run --example test_client -- -g all

# Test individual protocols
cargo run --example test_client -- -g http      # HTTP (10080)
cargo run --example test_client -- -g grpc      # gRPC (10080, HTTP/2)
cargo run --example test_client -- -g tcp       # TCP (19000)
cargo run --example test_client -- -g udp       # UDP (19002)
cargo run --example test_client -- -g websocket # WebSocket (10080)

# HTTPS tests (Gateway mode only)
cargo run --example test_client -- -g https     # HTTPS (10443)

# gRPC-TLS tests (Gateway mode only)
cargo run --example test_client -- -g grpc-tls  # gRPC over TLS (18443)

# TLS Terminate to TCP tests (Gateway mode only)
cargo run --example test_client -- -g tls-tcp   # TLS terminate (19001)

# mTLS tests (Gateway mode only)
cargo run --example test_client -- -g mtls      # Mutual TLS tests

# Plugin logging tests (Gateway mode only)
cargo run --example test_client -- -g plugin-logs  # Plugin execution validation

# Gateway mode + verbose output
cargo run --example test_client -- -g --verbose http

# Gateway mode + JSON report
cargo run --example test_client -- -g --json all
```

**Gateway Mode Features**:
- Automatically uses Gateway ports
- Automatically sets `Host: test.example.com` (for HTTP/HTTPS route matching)
- Automatically sets `Authority: grpc.example.com` (for gRPC route matching)
- Validates end-to-end gateway functionality (routing, load balancing, plugins, TLS)

### Test Suite Architecture

The test client uses a modular suite-based architecture:

**Location**: `examples/testing/test_client/suites/`

**Available Suites**:
- `http_suite.rs` - HTTP protocol tests (GET, POST, headers, status codes)
- `grpc_suite.rs` - gRPC unary and streaming tests
- `tcp_suite.rs` - TCP echo and connection tests
- `udp_suite.rs` - UDP datagram tests
- `websocket_suite.rs` - WebSocket bidirectional communication tests
- `https_suite.rs` - HTTPS with TLS termination tests
- `grpc_tls_suite.rs` - gRPC over TLS tests
- `tls_tcp_suite.rs` - TLS terminate to TCP backend tests
- `mtls_suite.rs` - Mutual TLS authentication tests
- `plugin_logs_suite.rs` - Plugin execution and logging validation tests

**Each Suite Contains**:
- Multiple test cases covering different scenarios
- Assertions for response validation
- Detailed error reporting
- Timing information for performance tracking

**Adding a New Test Suite**:
1. Create new file in `test_client/suites/`
2. Implement `TestSuite` trait
3. Add suite to `suites/mod.rs`
4. Register in `test_client.rs` Commands enum
5. Update this documentation

---

## Manual Testing Commands

### HTTP Tests (curl)

```bash
# Health check
curl -H "Host: test.example.com" http://localhost:10080/health

# Echo test
curl -H "Host: test.example.com" http://localhost:10080/echo

# Delay test
curl -H "Host: test.example.com" http://localhost:10080/delay/1

# API test
curl -H "Host: test.example.com" http://localhost:10080/api/users
```

### HTTPS Tests (curl)

```bash
# Health check (HTTPS)
curl -k -H "Host: test.example.com" \
  --resolve test.example.com:10443:127.0.0.1 \
  https://test.example.com:10443/secure/health

# Echo test (HTTPS)
curl -k -H "Host: test.example.com" \
  --resolve test.example.com:10443:127.0.0.1 \
  https://test.example.com:10443/secure/echo

# Status test (HTTPS)
curl -k -H "Host: test.example.com" \
  --resolve test.example.com:10443:127.0.0.1 \
  https://test.example.com:10443/secure/status/200
```

**Notes**:
- `-k` skips certificate verification (self-signed certificate)
- `--resolve` resolves domain to 127.0.0.1
- Must include both `-H "Host: test.example.com"` and `--resolve`

### gRPC Tests (grpcurl)

```bash
# Gateway HTTP mode (10080)
grpcurl -plaintext -authority grpc.example.com \
  -proto examples/proto/test_service.proto \
  -import-path . \
  localhost:10080 test.TestService/SayHello

# Gateway HTTP mode (with parameters)
grpcurl -plaintext -authority grpc.example.com \
  -proto examples/proto/test_service.proto \
  -import-path . \
  -d '{"name": "World"}' \
  localhost:10080 test.TestService/SayHello

# Gateway HTTPS mode (18443)
grpcurl -insecure -authority grpc.example.com \
  -proto examples/proto/test_service.proto \
  -import-path . \
  localhost:18443 test.TestService/SayHello

# Direct mode (30021)
grpcurl -plaintext \
  -proto examples/proto/test_service.proto \
  -import-path . \
  localhost:30021 test.TestService/SayHello
```

**Notes**:
- `-proto` specifies proto file path (test_server doesn't enable reflection API)
- `-import-path .` proto file import path
- `-authority grpc.example.com` sets `:authority` pseudo-header (required for Gateway mode)
- `-d '{"name": "World"}'` request parameters (JSON format)

### TCP Tests (nc/telnet)

```bash
# Gateway mode (19000)
(echo "Hello TCP"; sleep 0.5) | nc localhost 19000

# Or use -q parameter (supported on some systems)
echo "Hello TCP" | nc -q 1 localhost 19000

# Direct mode (30010)
(echo "Hello TCP"; sleep 0.5) | nc localhost 30010
```

**Notes**:
- `sleep 0.5` keeps connection open to receive response
- `-q 1` waits 1 second after EOF before closing (GNU netcat)

### UDP Tests (nc)

```bash
# Gateway mode (19002)
echo "Hello UDP" | nc -u localhost 19002

# Direct mode (30011)
echo "Hello UDP" | nc -u localhost 30011
```

### WebSocket Tests

```bash
# Using test_client (recommended)
cargo run --example test_client -- -g websocket  # Gateway mode
cargo run --example test_client -- websocket     # Direct mode

# Manual test with websocat
echo "Hello WebSocket" | websocat ws://localhost:10080/ws  # Gateway
echo "Hello WebSocket" | websocat ws://localhost:30005/ws  # Direct
```

---

## Port Reference

### test_server Backend Ports

| Protocol | Ports | Description |
|----------|-------|-------------|
| HTTP | 30001-30004 | HTTP test services (4 instances for load balancing) |
| gRPC | 30021 | gRPC test service |
| WebSocket | 30005 | WebSocket echo service |
| TCP | 30010 | TCP echo service |
| UDP | 30011 | UDP echo service |

### Gateway Listener Ports

| Protocol | Port | Description |
|----------|------|-------------|
| HTTP | 10080 | HTTP gateway |
| HTTPS | 10443 | HTTPS gateway (requires TLS certificates) |
| gRPC (HTTP) | 10080 | gRPC over HTTP/2 cleartext |
| gRPC (HTTPS) | 18443 | gRPC over TLS |
| TCP | 19000 | TCP proxy |
| UDP | 19002 | UDP proxy |
| TLS Terminate | 19001 | TLS termination to TCP backend |

### Controller Ports

| Port | Description |
|------|-------------|
| 50051 | gRPC API (Gateway connection) |
| 8080 | Admin API (health checks, resource queries) |

---

## TLS Certificates

HTTPS and gRPC-HTTPS tests require TLS certificates.

### Automatic Generation (Recommended)

```bash
cd examples/testing
./scripts/generate_certs.sh
```

### Generation Rules

- **Smart Skip**: Automatically skips if `Secret_edge_edge-tls.yaml` already exists
- **Regenerate On Demand**:
  ```bash
  rm ../conf/Secret_edge_edge-tls.yaml
  ./scripts/generate_certs.sh
  ```

### Certificate Details

- Self-signed certificate (test purposes only)
- Supports multiple domains (SAN):
  - `test.example.com` (HTTPS tests)
  - `grpc.example.com` (gRPC-HTTPS tests)
  - `tcp.example.com` (TLS Terminate to TCP tests)
- Temporary files automatically cleaned up (`/tmp/edgion-certs-$$`)
- Secret YAML ignored by `.gitignore`
- Client test certificate: `examples/testing/certs/ca.pem` (auto-generated)

### Generated Resources

```
examples/conf/
├── Secret_edge_edge-tls.yaml     # TLS certificate Secret
└── EdgionTls_edge_edge-tls.yaml  # TLS certificate config

examples/testing/certs/
└── ca.pem                         # CA certificate for client tests
```

---

## TLS Terminate to TCP Tests

This is a special test scenario to validate Gateway's TLS termination capability:

**Flow**: Client TLS → Gateway (terminates TLS) → Backend TCP (plaintext)

### Configuration Files

```yaml
# Gateway configuration (using annotation extension)
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tls-terminate-gateway
  namespace: edge
  annotations:
    edgion.io/backend-protocol: tcp  # Indicates TCP backend
spec:
  listeners:
    - name: tls-terminate-tcp
      protocol: TLS
      port: 19001
      tls:
        mode: Terminate
        certificateRefs:
          - name: edge-tls

# TLSRoute configuration (SNI-based routing)
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TLSRoute
metadata:
  name: test-tls-tcp
  namespace: edge
spec:
  parentRefs:
    - name: tls-terminate-gateway
      sectionName: tls-terminate-tcp
  hostnames:
    - "tcp.example.com"  # SNI hostname
  rules:
    - backendRefs:
        - name: test-tcp
          port: 30010
```

### Test Commands

```bash
# Using test_client (recommended)
cargo run --example test_client -- -g tls-tcp

# Manual test (using openssl)
(echo "TLS-TCP-TEST"; sleep 0.5) | \
  openssl s_client -connect 127.0.0.1:19001 \
  -servername tcp.example.com \
  -CAfile examples/testing/certs/ca.pem \
  -quiet 2>/dev/null
```

### Validation Points

1. ✅ TLS handshake successful (using correct SNI: tcp.example.com)
2. ✅ Gateway correctly terminates TLS
3. ✅ Backend receives plaintext TCP data
4. ✅ Echo response returns to client (encrypted via TLS)

### Use Cases

- Database proxy (e.g., MySQL, PostgreSQL)
- Redis TLS frontend
- Custom protocol TLS offloading
- Scenarios requiring traffic inspection/modification at Gateway layer

---

## Log Files

Integration test script log location: `examples/testing/logs/`

```
examples/testing/logs/
├── controller.log    # edgion-controller logs
├── gateway.log       # edgion-gateway logs
├── test_server.log   # test_server logs
├── access.log        # HTTP access logs
└── test_result.log   # Test result logs
```

### Viewing Logs

```bash
# Real-time Gateway access log
tail -f examples/testing/logs/access.log

# View test results
cat examples/testing/logs/test_result.log

# View Gateway logs
tail -f examples/testing/logs/gateway.log

# List all logs
ls -lh examples/testing/logs/
```

---

## Configuration Files

Configuration files are located in `examples/conf/`:

### Gateway API Resources

- `GatewayClass__public-gateway.yaml` - GatewayClass
- `Gateway_edge_example-gateway.yaml` - Gateway (HTTP/HTTPS/gRPC/TCP/UDP)
- `HTTPRoute_edge_test-http.yaml` - HTTP routes (includes WebSocket)
- `GRPCRoute_edge_test-grpc.yaml` - gRPC HTTP routes (10080)
- `GRPCRoute_edge_test-grpc-https.yaml` - gRPC HTTPS routes (18443)
- `TCPRoute_edge_test-tcp.yaml` - TCP routes
- `UDPRoute_edge_test-udp.yaml` - UDP routes

### Backend Services

- `Service_edge_test-*.yaml` - Service definitions
- `EndpointSlice_edge_test-*.yaml` - Backend endpoints

### TLS Resources

- `EdgionTls_edge_edge-tls.yaml` - TLS certificate configuration
- `Secret_edge_edge-tls.yaml` - TLS certificate data (auto-generated, gitignored)

### Edgion Custom Resources

- `EdgionPlugins_*.yaml` - HTTP plugin configurations (CORS, CSRF, IP restriction, etc.)
- `EdgionStreamPlugins_*.yaml` - Stream plugin configurations (TCP/UDP)
- `EdgionGatewayConfig__*.yaml` - Gateway-specific configurations
- `PluginMetaData_*.yaml` - Plugin metadata (IP lists, regex patterns, etc.)
- `LinkSys_*.yaml` - External system connections (Redis, databases, etc.)

---

## Troubleshooting

### Gateway Startup Fails

```bash
# Check if Controller is running
ps aux | grep edgion-controller

# Check Controller port
lsof -i :50051

# View Gateway logs
tail -100 examples/testing/logs/gateway.log
```

### HTTPS Tests Fail

```bash
# Check if certificate is generated
ls examples/conf/Secret_edge_edge-tls.yaml

# Regenerate certificate
rm examples/conf/Secret_edge_edge-tls.yaml
./scripts/generate_certs.sh

# Check HTTPS listener
lsof -i :10443
```

### Test Connection Fails

```bash
# Check all service processes
ps aux | grep -E "edgion|test_server"

# Check port usage
lsof -i :10080  # HTTP Gateway
lsof -i :10443  # HTTPS Gateway
lsof -i :30001  # HTTP Backend
```

### Configuration Load Failures

If `config_load_validator` reports failures:

1. **Check controller logs**:
   ```bash
   tail -100 examples/testing/logs/controller.log | grep -i error
   ```

2. **Common issues**:
   - YAML syntax errors (indentation, missing fields)
   - Invalid field names (check camelCase vs snake_case)
   - Missing required fields
   - Invalid enum values

3. **Validate YAML syntax**:
   ```bash
   # Install yamllint if needed
   yamllint examples/conf/YourFile.yaml
   ```

4. **Skip problematic resources temporarily**:
   Add annotation to the resource:
   ```yaml
   metadata:
     annotations:
       edgion.io/skip-load-validation: "true"
   ```

### Resource Synchronization Failures

If `resource_diff` reports mismatches:

1. **Wait for sync**: Resources may take a few seconds to propagate from controller to gateway
2. **Check gRPC connection**: Ensure gateway is connected to controller on port 50051
3. **Check controller logs**: Look for gRPC push errors
4. **Restart gateway**: Sometimes a restart resolves stale state

---

## For AI Assistants

### Quick Reference Guide

When working with Edgion testing framework, AI assistants should:

#### 1. Understanding the Test Pipeline

- **Primary entry point**: `examples/testing/run_integration_test.sh`
- **Test order**: Config validation → Resource sync → Direct mode → Gateway mode → Specialized tests
- **Two critical pre-checks**: 
  1. `config_load_validator` - Validates YAML configs loaded by controller
  2. `resource_diff` - Verifies controller-gateway state consistency

#### 2. Adding New Tests

**To add a new test suite**:
1. Create `examples/testing/test_client/suites/your_suite.rs`
2. Implement `TestSuite` trait with `run(&self, ctx: &TestContext) -> Result<TestResult>`
3. Add to `examples/testing/test_client/suites/mod.rs`
4. Register in `examples/testing/test_client.rs` Commands enum
5. Add to `run_integration_test.sh` if needed (usually for Gateway-only features)
6. Update this README.md

**Test case structure**:
```rust
fn test_something(&self, ctx: &TestContext) -> Result<TestCaseResult> {
    // 1. Send request
    let response = /* ... */;
    
    // 2. Assert expectations
    assert_eq!(response.status(), 200, "Status code mismatch");
    
    // 3. Return result
    Ok(TestCaseResult {
        name: "Test Something".to_string(),
        passed: true,
        message: None,
        duration: elapsed,
    })
}
```

#### 3. Adding New Configuration

**File naming convention**:
- With namespace: `Kind_namespace_name.yaml`
- Without namespace: `Kind__name.yaml` (double underscore)

**Special annotations**:
- Skip load validation: `edgion.io/skip-load-validation: "true"`

**After adding new configs**:
1. Place in `examples/conf/`
2. Run `cargo run --example config_load_validator` to verify
3. Run full integration test: `./run_integration_test.sh`

#### 4. Debugging Test Failures

**Systematic approach**:
1. **Check unified report**: `examples/testing/logs/test_result.log`
2. **Identify failing stage**: Config load? Resource sync? Specific test suite?
3. **View relevant logs**:
   - Controller: `examples/testing/logs/controller.log`
   - Gateway: `examples/testing/logs/gateway.log`
   - Test server: `examples/testing/logs/test_server.log`
   - Access logs: `examples/testing/logs/access.log`
4. **Run specific test**: `cargo run --example test_client -- -g <command>`
5. **Check resource state**: `cargo run --example resource_diff`

**Common failure patterns**:
- **502 Bad Gateway**: Backend not reachable, check ports and Service/EndpointSlice
- **404 Not Found**: Route not matched, check Host header and route paths
- **Configuration errors**: Check controller logs for YAML parsing errors
- **Plugin errors**: Check access.log for plugin execution details

#### 5. Port Allocation Rules

- **Backend (test_server)**: 30000-30999
- **Gateway listeners**: 10000-19999
- **Controller APIs**: 8080 (admin), 50051 (gRPC)

**Never use ports outside these ranges** to avoid conflicts.

#### 6. Plugin Testing

**Plugin log validation**:
- Plugin logs stored in `ctx.plugin_logs` (type: `Vec<StagePluginLogs>`)
- Structure: Array of stages, each containing plugin logs
- Stages: `request_filters`, `upstream_response_filters`, `upstream_responses`
- Each log has: `name`, `time_cost`, `log` (optional message)

**DebugAccessLogToHeader plugin**:
- Returns entire access log as JSON in `X-Debug-Access-Log` header
- Useful for testing plugin execution order and structure
- Example config: `EdgionPlugins_default_debug-access-log.yaml`

#### 7. Running Tests After Code Changes

**After modifying core routing/gateway logic**:
```bash
cd examples/testing
./run_integration_test.sh
```

**After modifying plugin system**:
```bash
cargo run --example test_client -- -g plugin-logs
```

**After modifying TLS handling**:
```bash
cargo run --example test_client -- -g https
cargo run --example test_client -- -g grpc-tls
cargo run --example test_client -- -g mtls
```

**Quick smoke test** (Direct mode, no setup needed):
```bash
cargo run --example test_server &
sleep 2
cargo run --example test_client -- all
killall test_server
```

#### 8. Key Files to Remember

- **Integration test script**: `examples/testing/run_integration_test.sh`
- **Config validator**: `examples/testing/config_load_validator.rs`
- **Resource diff**: `examples/testing/resource_diff.rs`
- **Test client**: `examples/testing/test_client.rs`
- **Test server**: `examples/testing/test_server.rs`
- **Test suites**: `examples/testing/test_client/suites/*.rs`
- **Configs**: `examples/conf/*.yaml`
- **Logs**: `examples/testing/logs/*.log`

#### 9. Best Practices

1. **Always run integration tests** after significant changes
2. **Check config_load_validator** output for YAML errors before debugging functional tests
3. **Verify resource_diff** passes before investigating routing issues
4. **Add test cases** for new features to prevent regressions
5. **Update this README** when adding new test suites or changing test infrastructure
6. **Keep test server running** when iterating on test client (faster feedback loop)
7. **Use verbose mode** (`-v`) when debugging specific test failures
8. **Check access.log** for plugin execution details and request/response information

#### 10. Understanding Test Infrastructure Evolution

The testing framework has evolved to include multiple validation layers:

**Historical context**:
- Initially: Only functional tests (Direct + Gateway modes)
- Added: Resource synchronization checks (`resource_diff`)
- Added: Configuration load validation (`config_load_validator`)
- Added: Plugin logging validation (`plugin_logs_suite`)
- Added: Unified test reporting

This layered approach catches issues early:
- Config errors caught by `config_load_validator` (before any routing tests)
- Sync issues caught by `resource_diff` (before functional tests)
- Functional issues caught by protocol-specific suites
- Plugin issues caught by dedicated plugin tests

**When adding new features**, consider:
1. Does it need new YAML resources? → Add example configs
2. Does it affect resource synchronization? → Verify `resource_diff` coverage
3. Does it need specialized validation? → Add new test suite
4. Does it change plugin behavior? → Update plugin log tests

---

**Last Updated**: 2025-12-27
**Test Framework Version**: v2.0 (with unified reporting and multi-layer validation)
