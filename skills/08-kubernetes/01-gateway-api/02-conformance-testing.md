---
name: gateway-api-conformance-testing
description: 如何运行、调试和逐步验证 Gateway API conformance tests
---

# Gateway API Conformance Testing

## 脚本位置

`examples/gateway-api-conformance/scripts/run.sh`

## 运行粒度

### 单个测试

```bash
./scripts/run.sh --run-test <TestName>
```

### 按 Profile

```bash
# HTTP
./scripts/run.sh --core --conformance-profiles GATEWAY-HTTP

# GRPC
./scripts/run.sh --conformance-profiles GATEWAY-GRPC \
  --supported-features "Gateway,GRPCRoute,ReferenceGrant"

# TLS
./scripts/run.sh --conformance-profiles GATEWAY-TLS \
  --supported-features "Gateway,TLSRoute,ReferenceGrant"
```

### 按 Feature 子集

```bash
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,HTTPRoutePathRedirect"
```

### 用 Go test 正则跑一批

```bash
cd examples/gateway-api-conformance
go test ./... -run "TestConformance/HTTPRouteRedirect" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch
```

### 预设

| 预设 | 命令 | 说明 |
|------|------|------|
| Core | `--core` | Gateway + HTTPRoute + ReferenceGrant |
| Extended | `--extended` | Core + 所有已实现的 Extended features |
| All | `--all-features` | 所有 feature（含未实现的，会有失败） |

## Port-Forward 模式

集群 Pod IP 不可达时使用：

```bash
./scripts/run.sh --port-forward --run-test HTTPRouteSimpleSameNamespace
```

自动 `kubectl port-forward` 到 edgion-gateway pod。

## Conformance Profiles 当前状态

| Profile | 声明 | 说明 |
|---------|------|------|
| GATEWAY-HTTP | ✅ | 已声明，核心功能已实现 |
| GATEWAY-GRPC | ⚠️ | 已声明，未实际验证 |
| GATEWAY-TLS | ❌ | 未声明，需通过 core tests 后再声明 |

## 调试失败的测试

1. **看日志**：`--no-debug` 关闭后默认有详细日志
2. **status condition 问题**：最常见的失败原因，检查 `handlers/gateway.rs` 的 `update_status()`
3. **超时**：`--timeout 60m` 延长，或看 `conformance_test.go` 中的 `TimeoutConfig`
4. **集群状态**：`kubectl get gateways,httproutes,tlsroutes -A` 看资源是否正确创建

## 任务追踪

- 验证进度：`tasks/todo/conformance-validation/`
- 缺失功能：`tasks/todo/missing-features/`
- 测试详情：`tasks/todo/gateway-api-1.5-conformance-tests/`
