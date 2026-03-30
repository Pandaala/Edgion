# GRPCExactMethodMatching

**Profile**: GATEWAY-GRPC Core
**状态**: FAIL
**根因类别**: gRPC 无匹配路由返回错误状态码

## 失败子测试

`2_request_to_'/gateway_api_conformance.echo_basic.grpcecho.GrpcEcho/EchoThree'_should_receive_a_Unimplemented_(12)`

其他 2 个子测试 (Echo → v1, EchoTwo → v2) 均通过。

## 错误现象

```
grpc.go:279: RPC finished with error: rpc error: code = Internal desc = server closed the stream without sending trailers
grpc.go:279: Received code Internal. Resetting connection.
grpc.go:285: Response expectation failed for request: expected status code to be Unimplemented (12), but got Internal (13)
```

## 根因分析

GRPCRoute 配置了两条精确方法匹配规则（Echo → v1, EchoTwo → v2）。当客户端请求 `EchoThree`（不匹配任何路由）时：

- **期望行为**: Gateway 应返回 gRPC status `Unimplemented (12)`，表示该方法未被路由
- **实际行为**: Gateway 直接关闭 stream 不发送 trailers，客户端收到 `Internal (13)`

这说明 **gateway 数据面在 gRPC 请求无匹配路由时没有正确发送 gRPC status trailers**，而是直接 reset/close 了连接。

## 修复方向

- **文件**: gateway 数据面的 gRPC proxy 模块
- **修改**: 当 gRPC 请求（`content-type: application/grpc`）无匹配路由时，应发送 gRPC trailers `grpc-status: 12` (Unimplemented) + `grpc-message: ...` 而不是关闭连接
- **参考**: Gateway API 规范要求无匹配 GRPCRoute 时返回 Unimplemented

## 关联

与 `02-GRPCRouteHeaderMatching`、`03-GRPCRouteListenerHostnameMatching` 同根因（无匹配路由→错误状态码）。
与 `04-GRPCRouteWeight` 相关但不同（Weight 测试期望 OK(0) 但也得到 Internal(13)，可能是完全不同的问题）。

## 复现命令

```bash
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/GRPCExactMethodMatching$" bash scripts/run.sh
```
