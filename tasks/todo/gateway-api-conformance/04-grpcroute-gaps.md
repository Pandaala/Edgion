# GRPCRoute 功能差距

优先级: P1

## 1. ~~RegularExpression 方法匹配~~ ✅ 已完成

已在 `GrpcMatchEngine` 中实现七级优先级匹配（Exact 优先于 Regex），正则模式预编译并按长度排序。集成测试通过。

改动文件：`match_engine.rs`、`match_unit.rs`
测试：`examples/test/conf/GRPCRoute/Match/GRPCRoute_edge_regex-*.yaml` + `matches.rs` 中 3 个 regex 测试用例

---

## 2. ~~未匹配请求返回 UNIMPLEMENTED~~ ✅ 已完成

在 `end_response_404` 中检测 `is_grpc_request`，gRPC 请求返回 `HTTP/2 200 + grpc-status: 12 (UNIMPLEMENTED) + grpc-message`，非 gRPC 请求保持原有 HTML 404。

改动文件：`src/core/gateway/runtime/server/error_response.rs`

---

## 3. ~~Hostname 唯一性（与 HTTPRoute 共享 listener）~~ ❌ 不实现

当前架构中 gRPC 请求先匹配 GRPCRoute、不匹配再 fallback HTTPRoute，优先级逻辑合理。同一 hostname 上 GRPCRoute + HTTPRoute 共存是正常使用模式，主流实现（Envoy Gateway、Istio）也不做跨类型冲突检测。
