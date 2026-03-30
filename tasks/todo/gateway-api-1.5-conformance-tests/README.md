# Gateway API v1.5 Conformance Tests — 失败项跟踪

测试日期: 2026-03-29
环境: OrbStack K8s v1.33.5, debug profile, Gateway API experimental CRDs v1.5.0

## 总览

### 全量模式（`bash scripts/run.sh`）

| Profile | Core Passed | Core Failed | Extended Passed | Extended Failed |
|---------|-------------|-------------|-----------------|-----------------|
| GATEWAY-HTTP | 33 | 0 | 24 | 4 |
| GATEWAY-GRPC | 9 | 4 | — | — |
| GATEWAY-TLS | 14 | 4 | — | — |

**总计**: 80 passed, 12 failed

### --tls-only 模式（`setup-orbstack.sh --tls-only` + `run.sh --conformance-profiles GATEWAY-TLS`）

| Profile | Core Passed | Core Failed |
|---------|-------------|-------------|
| GATEWAY-TLS | 17 | 1 |

TLS 部分从 14/18 提升到 **17/18**，3 个 TLS 失败确认为端口冲突环境问题，不是功能 bug。

## 失败分类

### 1. gRPC `Internal(13)` vs `Unimplemented(12)` — 4 个测试（真实 bug）

所有 4 个 GRPC 失败的**共同根因**：gateway 数据面在 gRPC 请求无匹配路由时直接关闭 stream（导致客户端收到 `Internal(13)`），应该返回 gRPC `Unimplemented(12)` status。

- `01-GRPCExactMethodMatching.md`
- `02-GRPCRouteHeaderMatching.md`
- `03-GRPCRouteListenerHostnameMatching.md`
- `04-GRPCRouteWeight.md`

### 2. TLS Passthrough 端口冲突 — 3 个测试（✅ --tls-only 已通过）

全量模式下 443 端口被 HTTPS terminate 占用，TLS passthrough 测试无法工作。
`--tls-only` 模式下全部通过，**不是功能 bug**。

- `05-TLSRouteSimpleSameNamespace.md` — ✅ PASS (--tls-only)
- `07-TLSRouteInvalidBackendRefNonexistent.md` — ✅ PASS (--tls-only)
- `08-TLSRouteInvalidBackendRefUnknownKind.md` — ✅ PASS (--tls-only)

### 3. TLS Hostname Intersection 路由 — 1 个测试（真实 bug）

`--tls-only` 模式仍失败。多 Gateway 共享 443 端口时 SNI hostname 路由不工作。

- `06-TLSRouteHostnameIntersection.md`

### 4. Frontend Client Certificate Validation — 3 个测试

动态创建的 Gateway 无法绑定 listener，或 per-port mTLS 逻辑问题。

- `09-GatewayFrontendClientCertificateValidation.md`
- `10-GatewayFrontendClientCertificateValidationInsecureFallback.md`
- `11-GatewayFrontendInvalidDefaultClientCertificateValidation.md`

### 5. HTTPS SNI/Host Mismatch 检测 — 1 个测试

- `12-HTTPRouteHTTPSListenerDetectMisdirectedRequests.md`

## 真实 bug 汇总（需要代码修复）

| # | 测试 | 根因 | 影响 |
|---|------|------|------|
| 1 | GRPC ×4 | gRPC 无匹配路由返回 Internal(13) 而非 Unimplemented(12) | 数据面 |
| 2 | TLSRouteHostnameIntersection | 多 Gateway 共享端口的 SNI hostname 路由 | 数据面/控制面 |
| 3 | mTLS ×3 | Frontend client certificate validation | 控制面/数据面 |
| 4 | Misdirected Requests ×1 | HTTPS SNI/Host 不匹配未返回 421 | 数据面 |

## 运行命令

```bash
# 全量测试（不需要 port-forward，OrbStack 直接访问 ClusterIP）
cd examples/gateway-api-conformance
bash scripts/run.sh

# TLS 专用模式
bash scripts/setup-orbstack.sh --tls-only --skip-build
bash scripts/run.sh --conformance-profiles GATEWAY-TLS

# 单测试
GO_RUN_FILTER="TestConformance/<TestName>$" bash scripts/run.sh

# 重建镜像 + 部署
bash scripts/orbstack-dev.sh rebuild

# 查看状态
bash scripts/orbstack-dev.sh status
```

## 注意事项

- OrbStack 下不需要 `--port-forward`，ClusterIP 可直接访问
- 全量测试不用加 `--cleanup`（预创建 Gateway 需要保留）
- 测试入口: `conformance_test.go`（feature 声明的唯一权威来源）
- 当前声明了 3 个 profile: GATEWAY-HTTP, GATEWAY-GRPC, GATEWAY-TLS
- `GO_RUN_FILTER` 单测模式的 conformance report 会把未运行的测试标记为 Passed，不可信
- TLS 测试需要 `--tls-only` 环境，与全量 HTTP/GRPC 测试不兼容（443 端口冲突）
