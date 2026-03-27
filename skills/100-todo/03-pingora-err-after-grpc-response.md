# Pingora Err 返回与 gRPC 响应的交互问题

## 状态：待研究

## 背景

在 `pg_upstream_peer.rs` 的 `upstream_peer_grpc` 函数中，当 gRPC upstream 无匹配路由或无 backend 时，流程为：

1. 调用 `end_response_grpc_unavailable()` 写入 gRPC trailers-only 响应（HTTP 200 + `grpc-status: 14`，`end_of_stream=true`）
2. 返回 `Err(PingoraError::new(ErrorType::InternalError))`

## 问题

`upstream_peer` 阶段返回 `Err` 后，Pingora 的 HTTP/2 层是否会额外发送 RST_STREAM，从而破坏步骤 1 中已经写好的 trailers-only 响应？

- HTTP/1.1 场景下不存在这个问题（没有 RST_STREAM 概念）
- HTTP/2 场景下，Pingora 对 `upstream_peer` 返回的 `Err` 的处理行为需要确认
- 这与 `end_response_grpc_unimplemented` 中移除 `session.shutdown()` 是同一类问题

## 代码位置

- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs:90-101` — 写响应后返回 Err
- `src/core/gateway/runtime/server/error_response.rs` — `end_response_grpc_unavailable`

## 验证方法

1. 部署后运行 gRPC conformance 测试，观察客户端收到的是 `grpc-status: 14` (UNAVAILABLE) 还是 RST_STREAM
2. 如果仍然是 RST_STREAM，需要改为不返回 Err，或者使用其他方式让 Pingora 跳过后续 proxy 阶段

## 可能的替代方案

- 在 ctx 中设置标志位标记"响应已发送"，在后续 Pingora 回调中检查并短路
- 返回一个哨兵 `HttpPeer`，配合 `upstream_request_filter` 阶段拦截，避免实际连接
- 研究 Pingora 是否有 `early_return` 或类似机制允许 `upstream_peer` 成功终止但不连接 upstream
