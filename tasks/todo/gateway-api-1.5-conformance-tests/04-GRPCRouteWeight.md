# GRPCRouteWeight

**Profile**: GATEWAY-GRPC Core
**状态**: FAIL
**根因类别**: gRPC 数据面代理完全不工作（可能与 01-03 不同）

## 失败子测试

`Requests_should_have_a_distribution_that_matches_the_weight`

## 错误现象

```
grpc.go:279: RPC finished with error: rpc error: code = Internal desc = server closed the stream without sending trailers
grpc.go:285: Response expectation failed for request:
  expected status code to be OK (0), but got Internal (13)
```

与 01-03 不同的是，这里**期望的是 OK(0)**（请求应该成功路由到后端），但也收到了 Internal(13)。这意味着 gRPC 请求到达 gateway 后完全无法被代理到后端。

## 根因分析

GRPCRoute 配置了 weight-based 后端分配（如 v1:50, v2:25, v3:25），客户端发送普通 gRPC Echo 请求应被路由到任一后端并收到 OK 响应。

**可能的原因**：
1. **与 01-03 同根因**：即使有匹配路由，gRPC 代理在建立到后端的 h2c 连接时也出了问题，关闭了 stream
2. **GRPCRoute 解析问题**：weight-based GRPCRoute 没有被正确解析到路由表中
3. **gRPC 代理本身有 bug**：`content-type: application/grpc` 的 HTTP/2 请求在 proxy 层处理不正确

### 排查思路

```bash
# 检查 GRPCRoute 是否被 controller 正确处理
kubectl -n gateway-conformance-infra get grpcroute weighted-backends -o yaml

# 检查 gateway 日志中 gRPC 请求的处理
./scripts/orbstack-dev.sh logs g

# 对比：HTTP 的 weight 测试（HTTPRouteWeight）是通过的，说明 weight 逻辑本身没问题
# 问题在 gRPC 协议特有的代理路径
```

## 修复方向

- 先修复 01 中的 gRPC Unimplemented 问题
- 然后重新测试此用例 — 如果根因相同，修复 01 后此测试可能直接通过
- 如果仍然失败，需要深入检查 gRPC proxy 模块中 HTTP/2 + `application/grpc` 的完整代理路径
- **关键对比**: HTTPRouteWeight 通过但 GRPCRouteWeight 失败 → gRPC 路由匹配或代理有独立的 bug

## 复现命令

```bash
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/GRPCRouteWeight$" bash scripts/run.sh
```
