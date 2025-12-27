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

### test_server 后端端口

| 协议 | 端口 | 说明 |
|------|------|------|
| HTTP | 30001-30004 | HTTP 测试服务（4 个实例）|
| gRPC | 30021 | gRPC 测试服务 |
| WebSocket | 30005 | WebSocket 回显服务 |
| TCP | 30010 | TCP 回显服务 |
| UDP | 30011 | UDP 回显服务 |

### Gateway 监听端口

| 协议 | 端口 | 说明 |
|------|------|------|
| HTTP | 10080 | HTTP 网关 |
| HTTPS | 10443 | HTTPS 网关（需要 TLS 证书）|
| gRPC (HTTP) | 10080 | gRPC over HTTP/2 |
| gRPC (HTTPS) | 18443 | gRPC over TLS |
| TCP | 19000 | TCP 代理 |
| UDP | 19002 | UDP 代理 |
| TLS Terminate | 19001 | TLS 终结到 TCP 后端 |

### Controller 端口

| 端口 | 说明 |
|------|------|
| 50051 | gRPC API（Gateway 连接）|
| 8080 | Admin API |

---

## TLS 证书

HTTPS 和 gRPC-HTTPS 测试需要 TLS 证书。

### 自动生成（推荐）

```bash
cd examples/testing
./scripts/generate_certs.sh
```

### 生成规则

- **智能跳过**：如果 `Secret_edge_edge-tls.yaml` 已存在，自动跳过
- **按需重新生成**：
  ```bash
  rm ../conf/Secret_edge_edge-tls.yaml
  ./scripts/generate_certs.sh
  ```

### 证书说明

- 自签名证书（仅用于测试）
- 支持多个域名（SAN）：
  - `test.example.com`（HTTPS 测试）
  - `grpc.example.com`（gRPC-HTTPS 测试）
  - `tcp.example.com`（TLS Terminate to TCP 测试）
- 临时文件自动清理（`/tmp/edgion-certs-$$`）
- Secret YAML 被 `.gitignore` 忽略
- 客户端测试证书：`examples/testing/certs/ca.pem`（自动生成）

### 生成的资源

```
examples/conf/
├── Secret_edge_edge-tls.yaml     # TLS 证书 Secret
└── EdgionTls_edge_edge-tls.yaml  # TLS 证书配置

examples/testing/certs/
└── ca.pem                         # 客户端测试用 CA 证书
```

---

## TLS Terminate to TCP 测试

这是一个特殊的测试场景，用于验证 Gateway 的 TLS 终结功能：

**流程**：客户端 TLS → Gateway（终结 TLS）→ 后端 TCP（明文）

### 配置文件

```yaml
# Gateway 配置（使用 annotation 扩展）
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tls-terminate-gateway
  namespace: edge
  annotations:
    edgion.io/backend-protocol: tcp  # 指示后端使用 TCP
spec:
  listeners:
    - name: tls-terminate-tcp
      protocol: TLS
      port: 19001
      tls:
        mode: Terminate
        certificateRefs:
          - name: edge-tls

# TLSRoute 配置（基于 SNI 路由）
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

### 测试命令

```bash
# 使用 test_client（推荐）
cargo run --example test_client -- -g tls-tcp

# 手动测试（使用 openssl）
(echo "TLS-TCP-TEST"; sleep 0.5) | \
  openssl s_client -connect 127.0.0.1:19001 \
  -servername tcp.example.com \
  -CAfile examples/testing/certs/ca.pem \
  -quiet 2>/dev/null
```

### 验证点

1. ✅ TLS 握手成功（使用正确的 SNI: tcp.example.com）
2. ✅ Gateway 正确终结 TLS
3. ✅ 后端收到明文 TCP 数据
4. ✅ Echo 响应能正确返回到客户端（通过 TLS 加密）

### 使用场景

- 数据库代理（如 MySQL、PostgreSQL）
- Redis TLS 前端
- 自定义协议的 TLS offloading
- 需要在 Gateway 层检查/修改流量的场景

---

## 日志文件

集成测试脚本日志位置：`examples/testing/logs/`

```
examples/testing/logs/
├── controller.log    # edgion-controller 日志
├── gateway.log       # edgion-gateway 日志
├── test_server.log   # test_server 日志
├── access.log        # HTTP 访问日志
└── test_result.log   # 测试结果日志
```

### 查看日志

```bash
# 实时查看 Gateway 访问日志
tail -f examples/testing/logs/access.log

# 查看测试结果
cat examples/testing/logs/test_result.log

# 查看 Gateway 日志
tail -f examples/testing/logs/gateway.log

# 查看所有日志
ls -lh examples/testing/logs/
```

---

## 配置文件

配置文件位于 `examples/conf/`：

### Gateway API 资源

- `GatewayClass__public-gateway.yaml` - GatewayClass
- `Gateway_edge_example-gateway.yaml` - Gateway（HTTP/HTTPS/gRPC/TCP/UDP）
- `HTTPRoute_edge_test-http.yaml` - HTTP 路由（包含 WebSocket）
- `GRPCRoute_edge_test-grpc.yaml` - gRPC HTTP 路由（10080）
- `GRPCRoute_edge_test-grpc-https.yaml` - gRPC HTTPS 路由（18443）
- `TCPRoute_edge_test-tcp.yaml` - TCP 路由
- `UDPRoute_edge_test-udp.yaml` - UDP 路由

### 后端服务

- `Service_edge_test-*.yaml` - Service 定义
- `EndpointSlice_edge_test-*.yaml` - 后端 Endpoint

### TLS 资源

- `EdgionTls_edge_edge-tls.yaml` - TLS 证书配置
- `Secret_edge_edge-tls.yaml` - TLS 证书数据（自动生成，被 gitignore）

---

## 故障排查

### Gateway 启动失败

```bash
# 检查 Controller 是否在运行
ps aux | grep edgion-controller

# 检查 Controller 端口
lsof -i :50051

# 查看 Gateway 日志
tail -100 examples/testing/logs/gateway.log
```

### HTTPS 测试失败

```bash
# 检查证书是否生成
ls examples/conf/Secret_edge_edge-tls.yaml

# 重新生成证书
rm examples/conf/Secret_edge_edge-tls.yaml
./scripts/generate_certs.sh

# 检查 HTTPS 监听器
lsof -i :10443
```

### 测试连接失败

```bash
# 检查所有服务进程
ps aux | grep -E "edgion|test_server"

# 检查端口占用
lsof -i :10080  # HTTP Gateway
lsof -i :10443  # HTTPS Gateway
lsof -i :30001  # HTTP Backend
```
