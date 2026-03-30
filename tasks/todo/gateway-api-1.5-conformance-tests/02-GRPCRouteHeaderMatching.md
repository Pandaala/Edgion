# GRPCRouteHeaderMatching

**Profile**: GATEWAY-GRPC Core
**状态**: FAIL
**根因类别**: gRPC 无匹配路由返回错误状态码

## 失败子测试

共 3 个子测试失败（请求带有不匹配 header 的场景）：

- `4_request_to_'/...GrpcEcho/Echo'_with_headers_'{Color:orange}'_should_receive_a_Unimplemented_(12)`
- `5_request_to_'/...GrpcEcho/Echo'_with_headers_'{Some-Other-Header:one}'_should_receive_a_Unimplemented_(12)`
- `10_request_to_'/...GrpcEcho/Echo'_with_headers_'{Color:purple}'_should_receive_a_Unimplemented_(12)`

匹配 header 的子测试（如 Color:blue → v1, Color:green → v2）均通过。

## 错误现象

```
grpc.go:279: RPC finished with error: rpc error: code = Internal desc = server closed the stream without sending trailers
grpc.go:285: Response expectation failed: expected status code to be Unimplemented (12), but got Internal (13)
```

## 根因分析

与 `01-GRPCExactMethodMatching` 完全相同的根因：gRPC 请求带有不匹配的 header 值时无匹配路由，gateway 直接关闭 stream 而不是发送 Unimplemented trailers。

**修复 01 即可同时解决此问题。**

## 修复方向

同 `01-GRPCExactMethodMatching.md` — gateway 数据面需要对无匹配路由的 gRPC 请求返回 `grpc-status: 12` trailers。

## 复现命令

```bash
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/GRPCRouteHeaderMatching$" bash scripts/run.sh
```
