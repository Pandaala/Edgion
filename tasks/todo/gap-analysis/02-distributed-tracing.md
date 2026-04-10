# 02 - Distributed Tracing (分布式追踪)

**优先级**: P0 - 生产必备

## 问题

Edgion 只有 access log 和 Prometheus metrics，缺少分布式追踪能力。在微服务架构下无法排查跨服务调用链路，网关成为可观测性的"黑洞"。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `opentelemetry` 插件（OTLP 导出）、`zipkin` 插件、`skywalking` 插件 |
| Kong | `opentelemetry` 插件（OTLP over HTTP/Protobuf）、`zipkin` 插件 |
| Envoy | 原生 OpenTelemetry / Zipkin / Jaeger tracer |

## 需要实现

- [ ] OpenTelemetry 集成（OTLP gRPC/HTTP 导出）
- [ ] 请求级 trace context 生成和传播（W3C Trace Context / B3 header）
- [ ] Span 记录关键节点（路由匹配、插件执行、upstream 请求、响应）
- [ ] 采样策略（固定比率、自适应采样）
- [ ] 与现有 access log 关联（trace_id 注入日志）
- [ ] 作为 HTTP 插件实现，可选择性开启

## 技术选型建议

- Rust 生态：`opentelemetry` crate + `opentelemetry-otlp`
- 与现有的 `tracing` / `tracing-subscriber` 集成
