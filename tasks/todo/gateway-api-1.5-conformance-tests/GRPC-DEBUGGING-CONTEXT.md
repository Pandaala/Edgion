# gRPC Conformance 失败调试上下文

## 背景

Gateway API v1.5 conformance 测试中 GATEWAY-GRPC profile 有 4 个 Core 测试失败。
01-03 是**同一根因**（无匹配路由时返回错误状态码），04 可能相同也可能不同。

## 环境搭建

```bash
cd examples/gateway-api-conformance

# 初始化环境（OrbStack 下，不需要 --tls-only）
bash scripts/setup-orbstack.sh --skip-build

# 确认环境正常
bash scripts/orbstack-dev.sh status

# 如果需要重新编译
bash scripts/orbstack-dev.sh rebuild
```

## 复现命令

```bash
cd examples/gateway-api-conformance

# 跑单个 GRPC 测试
GO_RUN_FILTER="TestConformance/GRPCExactMethodMatching$" bash scripts/run.sh
GO_RUN_FILTER="TestConformance/GRPCRouteHeaderMatching$" bash scripts/run.sh
GO_RUN_FILTER="TestConformance/GRPCRouteListenerHostnameMatching$" bash scripts/run.sh
GO_RUN_FILTER="TestConformance/GRPCRouteWeight$" bash scripts/run.sh

# 全部 GRPC 测试
GO_RUN_FILTER="TestConformance/GRPC" bash scripts/run.sh
```

**注意**: OrbStack 下不要加 `--port-forward`，ClusterIP 可直接访问。
全量测试 `go test` 约 12 分钟无输出是正常的（缓冲行为）。

## 失败现象

### 01-03: 无匹配路由 → 期望 Unimplemented(12)，实际收到 Internal(13)

测试场景：GRPCRoute 配置了部分匹配规则，客户端发送**不匹配**的请求。

```
期望: grpc-status: 12 (Unimplemented)
实际: code = Internal desc = server closed the stream without sending trailers
```

| 测试 | 场景 | 失败子测试 |
|------|------|-----------|
| GRPCExactMethodMatching | 方法 EchoThree 不匹配 | `EchoThree should receive Unimplemented(12)` |
| GRPCRouteHeaderMatching | header Color:orange/purple 不匹配 | 3 个子测试 |
| GRPCRouteListenerHostnameMatching | hostname foo.com/no.matching.host 不匹配 | 2 个子测试 |

匹配的子测试全部 PASS（说明 gRPC 路由匹配、代理本身是工作的）。

### 04: 匹配路由 → 期望 OK(0)，实际收到 Internal(13)

```
期望: grpc-status: 0 (OK)
实际: code = Internal desc = server closed the stream without sending trailers
```

GRPCRouteWeight 测试中请求应被 weight-based 路由到后端，但也收到 Internal(13)。
**先修 01-03，然后重测 04 — 可能同根因，也可能是独立 bug。**

## 代码定位

### 核心路径：gRPC 请求处理流程

```
请求进入
  → early_request_filter (协议发现: content-type=application/grpc → discover_protocol="grpc")
  → request_filter (pg_request_filter.rs)
      → try_match_grpc_route() 尝试匹配 gRPC 路由表
      → 如果不匹配且有 GRPCRoute 存在:
          end_response_grpc_unimplemented() → send_grpc_trailers_only(session, 501)
          return Ok(true)   ← 告诉 Pingora "响应已发送"
```

### 关键文件

1. **`src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:74-97`**
   gRPC 无匹配路由时的处理逻辑，已经写了 `end_response_grpc_unimplemented`。

2. **`src/core/gateway/runtime/server/error_response.rs:30-68`**
   `send_grpc_trailers_only`: HTTP 200 + grpc-status header + grpc-message header + END_STREAM=true
   `http_to_grpc_status`: 501 → `(12, "unimplemented")`

3. **`src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs:92-98`**
   **重要参考**: upstream_peer 阶段也有类似的 gRPC 错误响应处理，那里有一段关键注释：
   ```rust
   // gRPC trailers-only response already written above.
   // Return HTTPStatus(200) so Pingora treats this as "response already handled"
   // and does NOT send RST_STREAM.
   // InternalError would cause Pingora to reset the H2 stream,
   // and the client would see code=Internal(13) instead of Unavailable(14).
   return Err(PingoraError::new(ErrorType::HTTPStatus(200)));
   ```

4. **`src/core/gateway/routes/grpc/integration.rs`**
   `try_match_grpc_route`: 匹配逻辑，失败时返回 Ok(false)

5. **`src/core/gateway/routes/grpc/match_engine/`**
   gRPC 路由匹配引擎

## 最可能的根因

### 假设 A: Pingora H2 RST_STREAM 问题（最可能）

`pg_request_filter.rs` 中调用 `end_response_grpc_unimplemented` 后返回 `Ok(true)`。
而在 `pg_upstream_peer.rs` 的类似场景中，开发者特意注释说明：
**必须返回 `Err(HTTPStatus(200))` 而不是让流程正常结束，否则 Pingora 会发 RST_STREAM。**

但 `request_filter` 返回 `Ok(true)` 和 `upstream_peer` 返回 `Err(HTTPStatus(200))`
在 Pingora 框架里的行为可能不同。`request_filter` 返回 `Ok(true)` 后 Pingora 可能
仍然会做 stream 清理导致 RST_STREAM。

**排查方向**:
- 确认 `request_filter` 返回 `Ok(true)` 时 Pingora 对 H2 stream 的处理行为
- 可能需要在 `send_grpc_trailers_only` 后做额外的 session shutdown/cleanup
- 或者 `write_response_header(resp, true)` 的 `end_of_stream=true` 在 Pingora H2
  实现中没有正确发送 END_STREAM flag

### 假设 B: grpc_routes.has_routes() 返回 false

如果 gRPC 路由表为空（controller 没有正确同步 GRPCRoute 到 gateway），
那么条件 `!ctx.is_grpc_route_matched && grpc_routes.has_routes()` 不满足，
不会进入 unimplemented 分支，而是 fallthrough 到 HTTP 路由匹配。
HTTP 路由也不匹配，返回 404 HTML — 但测试看到的是 Internal(13) 而不是 404，
所以这个假设可能性较低。

### 假设 C: discover_protocol 不是 "grpc"

如果协议发现失败（比如 content-type header 解析问题），`is_grpc_request` 可能为 false，
请求直接走 HTTP 路由匹配路径。但匹配路由的子测试是通过的，说明至少在那些场景下
gRPC 协议发现是工作的。

## 调试建议

1. **加日志确认执行路径**:
   在 `pg_request_filter.rs:90-97` 的 unimplemented 分支加 tracing::info!，
   确认不匹配时是否真的走到了这个分支。

2. **抓包确认 H2 帧**:
   ```bash
   # 在 gateway pod 里用 tcpdump 抓 gRPC 请求
   kubectl -n edgion-system exec deploy/edgion-gateway -- tcpdump -i any -w /tmp/grpc.pcap port 80 &
   # 跑单测
   GO_RUN_FILTER="TestConformance/GRPCExactMethodMatching$" bash scripts/run.sh
   # 取出 pcap
   kubectl cp edgion-system/$(kubectl -n edgion-system get pod -l app=edgion-gateway -o jsonpath='{.items[0].metadata.name}'):/tmp/grpc.pcap /tmp/grpc.pcap
   ```
   用 Wireshark 看 H2 帧序列：是否有 HEADERS(200, grpc-status:12, END_STREAM) 后跟 RST_STREAM。

3. **查看 gateway admin API**:
   ```bash
   GW_IP=$(kubectl -n edgion-system get pod -l app=edgion-gateway -o jsonpath='{.items[0].status.podIP}')
   # 查看 gRPC 路由表是否有数据
   curl -s "http://${GW_IP}:5900/api/v1/debug/store-stats" | python3 -m json.tool
   # 查看 GRPCRoute 列表
   curl -s "http://${GW_IP}:5900/configclient/GRPCRoute/list" | python3 -m json.tool
   ```

4. **对比 upstream_peer 的处理方式**:
   `pg_upstream_peer.rs:92-98` 的模式（先写 trailers-only 再 `Err(HTTPStatus(200))`）是已验证可行的。
   可以参考这个模式修改 `request_filter` 中的 unimplemented 处理。

## 测试基础设施关键信息

- 测试入口: `conformance_test.go`
- gRPC echo server: conformance 框架自带，部署为 `grpc-infra-backend` Service
- 端口: gRPC 测试走 HTTP/2 on port 80（conformance 框架默认）
- gateway address: edgion-gateway Service ClusterIP（192.168.194.193，每次 setup 可能变）
- 测试框架发请求到 `{gateway_address}:80`，用 `content-type: application/grpc` + HTTP/2

## 文件索引

| 文件 | 内容 |
|------|------|
| `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs` | gRPC 路由匹配 + unimplemented 处理 |
| `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` | gRPC upstream 选择 + unavailable 处理 |
| `src/core/gateway/runtime/server/error_response.rs` | gRPC trailers-only 响应构建 |
| `src/core/gateway/routes/grpc/integration.rs` | gRPC 路由匹配入口 |
| `src/core/gateway/routes/grpc/match_engine/` | gRPC 路由匹配引擎 |
| `src/core/gateway/routes/grpc/routes_mgr.rs` | gRPC 路由表管理 |
| `src/core/gateway/routes/http/proxy_http/pg_early_request_filter.rs` | 协议发现 |
| `examples/gateway-api-conformance/conformance_test.go` | 测试入口 + feature 声明 |
| `tasks/todo/gateway-api-1.5-conformance-tests/01-GRPCExactMethodMatching.md` | 详细失败信息 |
