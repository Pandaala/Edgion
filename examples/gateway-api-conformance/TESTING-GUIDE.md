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

## Transport Access Layer (TAL)

TAL 是 Edgion conformance 测试的统一接入层，负责把"如何到达 Gateway"从"验证什么协议"中分离出来。

### 工作原理

TAL 只改写底层 TCP 连接的目标地址，不修改任何请求语义：
- Host header、`:authority`、URL path、TLS 参数、请求体均保持原样
- 地址改写有可选 debug 日志：`TAL_DEBUG=true`

```
[TAL] resolved 192.168.194.236:80 -> 127.0.0.1:18080
```

### 覆盖的协议场景

| 场景 | 官方扩展点 | TAL 组件 |
|------|------------|----------|
| HTTP/1.1 | `RoundTripper` | `tal.NewRoundTripper` |
| HTTPS / HTTP/2 over TLS | `RoundTripper` | `tal.NewRoundTripper` |
| H2C prior knowledge | `RoundTripper` | `tal.NewRoundTripper` |
| gRPC | `GRPCClient` | `tal.NewGRPCClient` |

### 已知边界（TAL 无法解决的问题）

- **WebSocket**：官方 conformance 框架尚无 dialer hook，TAL 暂无法拦截
- **TLSRoute 环境编排**：Gateway 启动前未创建正确 listener，TAL 无法弥补
- **Edgion 数据面 bug**：请求已到达 Gateway 但响应错误，需产品修复

### Port-Forward 模式

当集群 Pod IP 不可直接访问时（远程集群等场景）：

```bash
./scripts/run.sh --port-forward
./scripts/run.sh --port-forward --run-test HTTPRouteSimpleSameNamespace
```

> **OrbStack 用户注意**：OrbStack 原生支持 macOS 直连 K8s ClusterIP，
> **全量测试时不要加 `--port-forward`**。port-forward 在长时间运行（~12 分钟）
> 中可能断开，导致大量 `connection refused`。直接 `bash scripts/run.sh` 即可。

也可手动设置（推荐使用 `GATEWAY_PROXY_PORT_MAP` 统一管理端口映射）：

```bash
export GATEWAY_PROXY_HOST=127.0.0.1
# 方式 1：统一端口映射表（推荐）
export GATEWAY_PROXY_PORT_MAP="80:18080,443:18443,8443:28443"
# 方式 2：单独变量（兼容旧用法）
export GATEWAY_PROXY_HTTP_PORT=18080
export GATEWAY_PROXY_HTTPS_PORT=18443
export GATEWAY_PROXY_HTTPS_8443_PORT=28443

kubectl port-forward -n edgion-system pod/<gateway-pod> 18080:80 18443:443 28443:8443 &
./scripts/run.sh --run-test HTTPRouteSimpleSameNamespace
```

启用 gRPC proxy（`GRPCRoute*` 系列测试）：

```bash
GATEWAY_PROXY_ENABLE_GRPC=true ./scripts/run.sh --port-forward --conformance-profiles GATEWAY-GRPC
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

三个 profile 均已在 `conformance_test.go` 中声明（`opts.ConformanceProfiles`）。

| Profile | 声明 | Core | Extended | 备注 |
|---------|------|------|----------|------|
| GATEWAY-HTTP | ✅ | 33/33 passed | 24/28 passed | Core 全通过；Extended 4 failures (mTLS/421) |
| GATEWAY-GRPC | ✅ | 9/13 passed | — | 4 failures: gRPC 无匹配路由返回 Internal 而非 Unimplemented |
| GATEWAY-TLS | ✅ | 14/18 passed | — | 4 failures: 动态 TLS passthrough listener 不支持 |

> 数据截止：2026-03-29，全量 `bash scripts/run.sh`（直连，无 port-forward）

## 进度追踪

失败测试的逐项分析见：`tasks/todo/gateway-api-1.5-conformance-tests/`

每个失败测试一个文件，包含错误日志、根因分析、修复方向和复现命令。
