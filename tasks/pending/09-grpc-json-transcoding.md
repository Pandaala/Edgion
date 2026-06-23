# 09 - gRPC-JSON Transcoding (gRPC-JSON 转码)

**优先级**: P1 - 竞品标配

## 问题

RESTful 客户端（浏览器、移动端、curl）无法直接调用 gRPC 服务。gRPC-JSON 转码让网关自动将 HTTP/JSON 请求转为 gRPC 调用，免去客户端 protobuf 依赖。在 gRPC 生态中非常实用。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `grpc-transcode` 插件，上传 proto 文件后自动转换 |
| Kong | gRPC-REST Gateway（通过 proto 定义映射） |
| Envoy | 原生 gRPC-JSON Transcoder Filter，基于 proto descriptor |

## 需要实现

- [ ] Proto 文件/descriptor 管理（上传、存储、版本管理）
- [ ] HTTP/JSON → gRPC 请求转换（path 映射、body → protobuf 序列化）
- [ ] gRPC → HTTP/JSON 响应转换（protobuf → JSON 反序列化）
- [ ] 遵循 google.api.http 注解的路径映射规则
- [ ] 错误码映射（gRPC status → HTTP status）
- [ ] Streaming 支持（至少支持 unary 和 server-streaming）
- [ ] 作为 HTTP 插件或 GRPCRoute 扩展实现

## 技术选型建议

- `prost` / `prost-reflect` crate 用于动态 protobuf 处理
- 参考 Envoy 的 google.api.http 注解规范
