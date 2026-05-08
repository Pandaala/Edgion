---
name: conformance-workflow
description: 跨会话工作流：逐项推进 Gateway API conformance 测试，包含环境检查、功能分析、测试运行、诊断和结果记录
---

# Gateway API Conformance Workflow

逐项跑通 Gateway API conformance 测试的标准工作流。每次新会话都可以从这里开始。

## 前置条件

OrbStack 环境已通过 `setup-orbstack.sh` 搭建完成。如果尚未搭建：

```bash
cd examples/gateway-api-conformance
./scripts/setup-orbstack.sh
```

## 工作流步骤

### 1. 环境检查

```bash
cd examples/gateway-api-conformance
./scripts/orbstack-dev.sh status
```

检查项：
- kubectl context = orbstack
- edgion-controller Pod Running
- edgion-gateway Pod Running
- GatewayClass edgion Accepted=True
- Gateway same-namespace 有 Address

如果有问题，先排查再继续。

### 2. 选择测试项

进度 checklist 由用户/维护者按需自行维护（仓库内目前不放置常驻文件）。

优先级顺序：
1. HTTPRoute Core（未勾选项）
2. Gateway Core（未勾选项）
3. HTTPRoute Extended
4. Gateway Extended
5. TLSRoute / GRPCRoute

用户也可以直接指定某个测试。

### 3. 功能分析

对选中的测试进行分析。读取：

1. 测试源代码 → `examples/gateway-api-conformance/conformance_test.go` 与上游 `sigs.k8s.io/gateway-api/conformance/tests/<test-name>.go`（按测试名直接 grep）
2. 已知不支持项 → `skills/08-kubernetes/01-gateway-api/01-not-implemented.md`
3. Controller 实现 → `skills/01-architecture/01-controller/`
4. Gateway 数据面 → `skills/01-architecture/02-gateway/`

分析输出格式：
```
测试: <TestName>
Feature: <FeatureName>
Level: Core/Extended
预期: PASS / FAIL(需改动) / SKIP(不实现)
分析: <简要说明>
所需改动: <如果需要>
```

### 4. 运行测试

```bash
./scripts/orbstack-dev.sh test --run-test <TestName>
```

或直接用 Go test（更灵活的正则匹配）：

```bash
cd examples/gateway-api-conformance
go test ./... -run "TestConformance/<TestName>" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch
```

### 5. 诊断失败

如果测试失败：

1. **分析 go test 输出**：看具体哪个断言失败
2. **查看 controller 日志**：`./scripts/orbstack-dev.sh logs c`
3. **查看 gateway 日志**：`./scripts/orbstack-dev.sh logs g`
4. **检查 K8s 资源状态**：
   ```bash
   kubectl -n gateway-conformance-infra get gateway <name> -o yaml
   kubectl -n gateway-conformance-infra get httproute -o yaml
   ```
5. 定位 root cause：controller status？路由匹配？数据面转发？
6. **批量跑失败但单跑通过？** → 参考 `05-batch-conformance-debugging.md` 进行三路日志采集和时序关联分析

### 6. 修复 → 重建 → 重测

```bash
# 修改代码后...

# 重建镜像 + rollout（最可靠）
./scripts/orbstack-dev.sh rebuild c    # 只重建 controller
./scripts/orbstack-dev.sh rebuild g    # 只重建 gateway
./scripts/orbstack-dev.sh rebuild      # 两个都重建

# 或 hotpatch（最快，cp binary + 杀进程）
./scripts/orbstack-dev.sh hotpatch c

# 重新运行测试
./scripts/orbstack-dev.sh test --run-test <TestName>
```

### 7. 记录结果

测试通过后：
1. 在自己维护的进度 checklist 中勾选（仓库内不放置常驻文件）
2. 如有重要发现，更新 `skills/08-kubernetes/01-gateway-api/` 相关文档

### 8. 扩展运行（可选）

```bash
./scripts/orbstack-dev.sh test --core                    # Core 测试
./scripts/orbstack-dev.sh test --extended                # Extended 测试
./scripts/orbstack-dev.sh test --supported-features "Gateway,HTTPRoute,ReferenceGrant,HTTPRouteQueryParamMatching"
```

## 常见问题排查

| 症状 | 可能原因 | 解决 |
|------|---------|------|
| GatewayClass not Accepted | Controller 未启动或 crash | `orbstack-dev.sh logs c` |
| Gateway no address | gateway_address 配置错误 | 重新运行 setup-orbstack.sh |
| HTTP/gRPC test timeout | Pod IP 不可达 | 用 in-cluster 模式（推荐）或 `--port-forward` |
| gRPC tests hang/crash | port-forward 不支持 gRPC 长连接 | 必须用 in-cluster 模式 |
| 测试 SKIP | feature 未在 supported-features 中声明 | 检查 conformance_test.go |
| Pod CrashLoopBackOff | Binary crash | `orbstack-dev.sh logs c/g` |

详细排障手册见 [06-conformance-debug-cookbook.md](06-conformance-debug-cookbook.md)。

## 相关文件

- 测试入口: `examples/gateway-api-conformance/conformance_test.go`
- 运行脚本: `examples/gateway-api-conformance/scripts/run.sh`
- 不实现项: `skills/08-kubernetes/01-gateway-api/01-not-implemented.md`
