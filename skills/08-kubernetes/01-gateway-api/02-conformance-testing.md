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
| `MaxTimeToConsistency` | 15s | 30s |
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

### Port-Forward 模式（TAL）

集群 Pod IP 不可达时使用。TAL（Transport Access Layer）负责统一处理地址改写，支持灵活的端口映射配置。

**快速启用：**

```bash
bash scripts/run.sh --port-forward
```

自动启动 `kubectl port-forward` 到 edgion-gateway pod，默认绑定本地端口（HTTP:18080, HTTPS:18443, HTTPS-8443:28443）。

**高级用法 — 统一端口映射表（推荐）：**

```bash
export GATEWAY_PROXY_PORT_MAP="80:18080,443:18443,8443:28443"
./scripts/run.sh --port-forward
```

`GATEWAY_PROXY_PORT_MAP` 格式：`gwPort:localPort,gwPort:localPort,...`，允许为每个 gateway 端口独立配置本地转发端口。

**兼容旧用法（逐个指定）：**

```bash
export GATEWAY_PROXY_HOST=127.0.0.1
export GATEWAY_PROXY_HTTP_PORT=18080
export GATEWAY_PROXY_HTTPS_PORT=18443
export GATEWAY_PROXY_HTTPS_8443_PORT=28443
./scripts/run.sh --port-forward
```

**启用 gRPC proxy：**

```bash
GATEWAY_PROXY_ENABLE_GRPC=true ./scripts/run.sh --port-forward
```

用于 `GRPCRoute*` 系列测试。

**调试 TAL 地址改写：**

```bash
TAL_DEBUG=true ./scripts/run.sh --port-forward
```

在 test 输出中查看 `[TAL] resolved ...` 日志。

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

Gateway API 规范定义 `Conflicted` 为 **完全相同** 的 socket-address+hostname 组合（冲突检测 key 通过 `make_conflict_key(SocketAddr)` 按 socket 地址分组），不是 hostname 相交。

- `None`（wildcard）+ `None` → **Conflicted**（两个都是 wildcard）
- `foo.example.com` + `foo.example.com` → **Conflicted**
- `None` + `foo.example.com` → **不冲突**（相交但不同）
- `*.example.com` + `foo.example.com` → **不冲突**（相交但不同）

错误地将"相交"视为"冲突"会导致多个测试失败（`GatewayHTTPListenerIsolation` 等）。

## Conformance 最新结果（feature-03-29, 2026-04-01）

**381 PASS / 7 FAIL / 56 SKIP**（in-cluster 模式）

详见 `SKILL.md` 中的 FAIL 列表和 profile 状态。

## In-Cluster 运行方式（推荐）

OrbStack 路由不稳定时用 in-cluster 方式跑，避免 port-forward 和网络问题：

```bash
# 1. 编译 linux/arm64 binary
cd examples/gateway-api-conformance
GOOS=linux GOARCH=arm64 CGO_ENABLED=0 go test -c -o /tmp/conformance-test-linux .

# 2. 创建 runner pod（用已有镜像避免 Docker Hub 限流）
kubectl apply -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: conformance-runner
  namespace: edgion-system
spec:
  containers:
  - name: runner
    image: pandaala/edgion-controller:conformance-dev
    imagePullPolicy: IfNotPresent
    command: ["sh", "-c", "sleep infinity"]
  restartPolicy: Never
  serviceAccountName: edgion-controller
EOF

# 3. 授权 cluster-admin（conformance suite 需要操作 CRD/namespace）
kubectl apply -f - <<'EOF'
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: conformance-runner-admin
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: cluster-admin
subjects:
- kind: ServiceAccount
  name: edgion-controller
  namespace: edgion-system
EOF

# 4. 拷贝 binary 并启动（nohup 防止 kubectl exec 断开杀进程）
kubectl cp /tmp/conformance-test-linux edgion-system/conformance-runner:/tmp/conformance-test
kubectl exec -n edgion-system conformance-runner -- chmod +x /tmp/conformance-test
kubectl exec -n edgion-system conformance-runner -- sh -c '
nohup /tmp/conformance-test \
  -test.run=TestConformance -test.v -test.timeout=30m -test.count=1 \
  -gateway-class=edgion -cleanup-base-resources=false -debug=true \
  -allow-crds-mismatch -conformance-profiles=GATEWAY-HTTP,GATEWAY-GRPC,GATEWAY-TLS \
  > /tmp/conformance.log 2>&1 &
echo "PID=$!"'

# 5. 监控
kubectl exec -n edgion-system conformance-runner -- sh -c \
  'wc -l /tmp/conformance.log; tail -3 /tmp/conformance.log; ls /proc/$(pgrep conformance)/stat 2>/dev/null && echo alive || echo done'

# 6. 取回日志
kubectl cp edgion-system/conformance-runner:/tmp/conformance.log /tmp/conformance-final.log
```
