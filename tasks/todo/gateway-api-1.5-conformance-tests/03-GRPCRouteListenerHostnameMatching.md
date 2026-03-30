# GRPCRouteListenerHostnameMatching

**Profile**: GATEWAY-GRPC Core
**状态**: FAIL
**根因类别**: gRPC 无匹配路由返回错误状态码

## 失败子测试

共 2 个子测试失败（请求发往不匹配 hostname 的场景）：

- `6_request_to_'foo.com/...GrpcEcho/Echo'_should_receive_a_Unimplemented_(12)`
- `7_request_to_'no.matching.host/...GrpcEcho/Echo'_should_receive_a_Unimplemented_(12)`

匹配 hostname 的子测试（如 bar.com → v2, foo.bar.com → v3 等）均通过。

## 错误现象

```
grpc.go:279: RPC finished with error: rpc error: code = Internal desc = server closed the stream without sending trailers
grpc.go:279: Received code Internal. Resetting connection.
```

## 根因分析

与 `01-GRPCExactMethodMatching` 完全相同的根因：hostname 不匹配任何 listener 时无匹配路由，gateway 直接关闭 stream。

**修复 01 即可同时解决此问题。**

## 修复方向

同 `01-GRPCExactMethodMatching.md`。

## 复现命令

```bash
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/GRPCRouteListenerHostnameMatching$" bash scripts/run.sh
```
