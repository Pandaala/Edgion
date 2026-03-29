# Gateway API Conformance Testing Guide

## Quick Start

```bash
# 跑 HTTP Core（最基本的验证）
./scripts/run.sh --core

# 跑单个测试
./scripts/run.sh --run-test HTTPRouteSimpleSameNamespace

# 跑 Extended
./scripts/run.sh --extended
```

## TLSRoute 专用环境

TLSRoute data-plane conformance 需要在 Gateway 进程启动前就把 443 上的 TLS
passthrough listener 预创建好。默认的 `conformance-gateway.yaml` 预创建了
HTTPS 443 listener，会和 TLSRoute 的 443 passthrough listener 冲突。

因此，跑 TLSRoute data-plane 测试前请先用 TLS 专用 setup：

```bash
./scripts/setup-orbstack.sh --tls-only
```

然后再跑 TLSRoute 测试。OrbStack 下通常不需要 `--port-forward`：

```bash
GO_RUN_FILTER="TestConformance/TLSRouteSimpleSameNamespace$" \
  bash scripts/run.sh --conformance-profiles GATEWAY-TLS
```

## 测试粒度控制

### 单个测试

```bash
./scripts/run.sh --run-test <TestName>
```

### 按 Conformance Profile

```bash
# 只跑 HTTP profile
./scripts/run.sh --conformance-profiles GATEWAY-HTTP

# 只跑 GRPC profile
./scripts/run.sh --conformance-profiles GATEWAY-GRPC \
  --supported-features "Gateway,GRPCRoute,ReferenceGrant"

# 只跑 TLS profile
./scripts/run.sh --conformance-profiles GATEWAY-TLS \
  --supported-features "Gateway,TLSRoute,ReferenceGrant"

# 多个 profile
./scripts/run.sh --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC"
```

### 按 Feature 子集

```bash
# 只跑重定向相关
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,HTTPRoutePathRedirect,HTTPRoutePortRedirect,HTTPRouteSchemeRedirect"

# 只跑 mirror 相关
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,HTTPRouteRequestMirror,HTTPRouteRequestMultipleMirrors"
```

### 用 Go test 正则匹配

```bash
cd examples/gateway-api-conformance

# 匹配名称包含 "Redirect" 的所有测试
go test ./... -run "TestConformance/.*Redirect" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch

# 匹配名称包含 "TLSRoute" 的所有测试
go test ./... -run "TestConformance/TLSRoute" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch
```

### 预设模式

```bash
./scripts/run.sh --core          # Gateway + HTTPRoute + ReferenceGrant
./scripts/run.sh --extended      # Core + 所有已实现 Extended features
./scripts/run.sh --all-features  # 所有 feature（含未实现的）
```

## Port-Forward 模式

当集群 Pod IP 不可直接访问时：

```bash
./scripts/run.sh --port-forward
./scripts/run.sh --port-forward --run-test HTTPRouteSimpleSameNamespace
```

也可手动设置：

```bash
export GATEWAY_PROXY_HOST=127.0.0.1
export GATEWAY_PROXY_HTTP_PORT=8080
export GATEWAY_PROXY_HTTPS_PORT=8443
kubectl port-forward -n edgion-system pod/<gateway-pod> 8080:80 8443:443 &
./scripts/run.sh --run-test HTTPRouteSimpleSameNamespace
```

## 所有选项

```
--gateway-class <name>        GatewayClass 名称 (default: edgion)
--supported-features <list>   逗号分隔的 feature 列表
--conformance-profiles <list> 逗号分隔的 profile 列表
--run-test <name>             运行单个测试
--all-features                启用所有 features
--core                        仅 Core features
--extended                    Core + Extended features
--cleanup                     测试后清理
--no-debug                    关闭调试输出
--timeout <duration>          超时时间 (default: 30m)
--report <path>               报告输出路径
--port-forward                自动 port-forward
```

## Conformance Profiles

| Profile | 状态 | 测试数 |
|---------|------|--------|
| GATEWAY-HTTP | ✅ 已声明 | ~75 |
| GATEWAY-GRPC | ⚠️ 已声明未验证 | 5 |
| GATEWAY-TLS | ❌ 未声明 | 13 |

## 进度追踪

详细的逐测试进度追踪见：
- `tasks/todo/conformance-validation/` — 已有功能验证
- `tasks/todo/missing-features/` — 缺失功能实现
