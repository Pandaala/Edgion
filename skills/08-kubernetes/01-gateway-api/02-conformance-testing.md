---
name: gateway-api-conformance-testing
description: 如何运行、调试和逐步验证 Gateway API conformance tests
---

# Gateway API Conformance Testing

## 脚本位置

`examples/gateway-api-conformance/scripts/run.sh`

## Feature 声明权威来源

**`conformance_test.go` 是 supported features 的唯一权威来源。**

`run.sh` 默认不传 `-supported-features`，让 `conformance_test.go` 里的 `opts.SupportedFeatures` 生效。
只有显式传 `--core`、`--extended`、`--supported-features <list>` 时才覆盖。

新增 feature 支持时：
1. 在 `conformance_test.go` 里 `opts.SupportedFeatures.Insert(features.SupportXxx)` 添加
2. `run.sh` 无需改动

## 超时配置

`conformance_test.go` 统一配置了短超时（适用于快速响应的本地集群）：

| 参数 | 配置值 | 库默认值 |
|------|--------|---------|
| `GatewayMustHaveAddress` | 10s | 180s |
| `GatewayMustHaveCondition` | 10s | 180s |
| `GatewayStatusMustHaveListeners` | 10s | 60s |
| `HTTPRouteMustHaveCondition` | 10s | 60s |
| `RouteMustHaveParents` | 10s | 60s |
| `MaxTimeToConsistency` | 30s | 30s |
| `LatestObservedGenerationSet` | 10s | 60s |
| `DefaultTestTimeout` | 15s | 60s |
| `RequiredConsecutiveSuccesses` | 3 | 3 |

**重要**：如果测试等待超时（10s 后 context deadline exceeded），根因几乎总是 status condition 设置错误，而非超时太短。控制器正常时状态在 1s 内更新。

## 运行粒度

### 单个测试

```bash
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/GatewayObservedGenerationBump" bash scripts/run.sh
```

`GO_RUN_FILTER` 转换为 `go test -run <filter>`，在 profile 模式下正确过滤子测试（`--run-test` 在 profile 模式下无效）。

### 按名称前缀跑一批

```bash
# 所有 Gateway/GatewayClass 测试
GO_RUN_FILTER="TestConformance/Gateway" bash scripts/run.sh

# 所有 HTTPRoute 测试
GO_RUN_FILTER="TestConformance/HTTPRoute" bash scripts/run.sh
```

### 预设

| 预设 | 命令 | 说明 |
|------|------|------|
| 默认（全部声明 features） | `bash scripts/run.sh` | 使用 conformance_test.go 里声明的全套 features |
| Core only | `--core` | 强制只跑 Gateway+HTTPRoute+ReferenceGrant |
| Extended | `--extended` | 扩展 feature 集合（含 redirect、mirror 等） |
| All features | `--all-features` | 所有 feature（含未实现的，会有 FAIL） |

### 单独指定 features（覆盖 conformance_test.go）

```bash
bash scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant"
```

### Port-Forward 模式

集群 Pod IP 不可达时使用：

```bash
bash scripts/run.sh --port-forward
```

自动 `kubectl port-forward` 到 edgion-gateway pod，HTTP→8080, HTTPS→8443。

## 更新二进制（localpatch）

```bash
# 只更新 controller
./examples/gateway-api-conformance/scripts/orbstack-dev.sh localpatch c

# 更新 controller + gateway
./examples/gateway-api-conformance/scripts/orbstack-dev.sh localpatch
```

**注意 Pod 进程结构**：PID 1 是 shell loop（`while true; do /usr/local/edgion/edgion-controller; done`），不要 kill PID 1，否则 OrbStack 会用原始镜像重启容器（丢失已复制的新二进制）。
应 kill controller 子进程，让 shell loop 用新二进制重启：

```bash
POD=$(kubectl get pods -n edgion-system -l app=edgion-controller -o jsonpath='{.items[0].metadata.name}')
CTRL_PID=$(kubectl exec -n edgion-system $POD -- sh -c \
  "for p in \$(ls /proc | grep '^[0-9]*$'); do \
    if [ -f /proc/\$p/cmdline ] && grep -q edgion-controller /proc/\$p/cmdline 2>/dev/null && [ \$p != 1 ]; then echo \$p; fi; \
  done" | head -1)
kubectl exec -n edgion-system $POD -- sh -c "kill -9 $CTRL_PID"
```

## Status Condition 调试

**大多数 conformance test 失败根因是 status condition 设置错误**，而非路由/数据面问题。

### 调试流程

1. 跑单个失败测试，看 `context deadline exceeded` 的具体 condition：
   ```
   Programmed condition set to Status False with Reason Invalid, expected Status True
   ```

2. 检查集群中资源的实际 status：
   ```bash
   kubectl get gateway <name> -n gateway-conformance-infra -o yaml | grep -A 30 "status:"
   ```

3. 对照 `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs` 的 `update_status()` 和 `update_listener_conditions()` 逻辑

### 常见 Status 问题

| 症状 | 根因 | 修复位置 |
|------|------|---------|
| `Programmed=False(Invalid)` 但预期 `True` | 冲突检测误判 / unresolved refs 逻辑问题 | `listener_port_manager.rs` / `update_listener_conditions()` |
| `Programmed=True` 但预期 `False` | 未将 `has_unresolved_refs` 纳入 `Programmed` 判断 | `update_listener_conditions()` |
| `AttachedRoutes=0` 但预期 `>0` | route tracker 未计数 / gateway 有 unresolved refs 时未计 | `update_status()` 里的 `tracker.count_for_listener()` |
| `ResolvedRefs=False(RefNotPermitted)` 误判 | ReferenceGrant 解析逻辑 | `validate_listener_resolved_refs()` |

### Hostname 冲突检测规则（重要）

Gateway API 规范定义 `Conflicted` 为 **完全相同** 的 port+protocol+hostname 组合，不是 hostname 相交。

- `None`（wildcard）+ `None` → **Conflicted**（两个都是 wildcard）
- `foo.example.com` + `foo.example.com` → **Conflicted**
- `None` + `foo.example.com` → **不冲突**（相交但不同）
- `*.example.com` + `foo.example.com` → **不冲突**（相交但不同）

错误地将"相交"视为"冲突"会导致多个测试失败（`GatewayHTTPListenerIsolation` 等）。

## Conformance Profiles 当前状态

| Profile | 声明 | 说明 |
|---------|------|------|
| GATEWAY-HTTP | ✅ | 已声明，Gateway + HTTPRoute core 已验证 |
| GATEWAY-GRPC | ✅ | 已声明，GRPCRoute core 已验证 |
| GATEWAY-TLS | ❌ | 未声明，需通过 core tests 后再声明 |

## Gateway 测试当前状态（feature-03-25 分支）

| 测试 | 状态 | 备注 |
|------|------|------|
| GatewayClassObservedGenerationBump | ✅ | |
| GatewayInvalidRouteKind | ✅ | |
| GatewayInvalidTLSConfiguration（4个子测试） | ✅ | |
| GatewayModifyListeners | ✅ | |
| GatewayObservedGenerationBump | ✅ | |
| GatewaySecretInvalidReferenceGrant | ✅ | |
| GatewaySecretMissingReferenceGrant | ✅ | |
| GatewaySecretReferenceGrantAllInNamespace | ✅ | |
| GatewaySecretReferenceGrantSpecific | ✅ | |
| GatewayWithAttachedRoutes（3个子测试） | ✅ | |
| GatewayHTTPListenerIsolation | ✅ | |
| GatewayFrontendInvalidDefaultClientCertificateValidation | ✅ | |
| GatewayFrontendClientCertificateValidation | ❌ | status condition 问题 |
| GatewayFrontendClientCertificateValidationInsecureFallback | ❌ | status condition 问题 |
| GatewayInvalidFrontendClientCertificateValidation | ❌ | `default_valid_configuration` 子测试 status 问题 |
| GatewayStaticAddresses | ⏭ SKIP | 明确不实现（地址由 TOML 配置决定） |
| GatewayOptionalAddressValue | ⏭ SKIP | 明确不实现 |
| GatewayWithAttachedRoutesWithPort8080 | ⏭ SKIP | 明确不实现（端口只能启动时绑定） |
| GatewayInfrastructure | ⏭ SKIP | 明确不实现 |
| GatewayBackendClientCertificateFeature | ⏭ SKIP | 需要 BackendTLSPolicy CRD |
| GatewayInvalidTLSBackendConfiguration | ⏭ SKIP | 需要 BackendTLSPolicy CRD |
