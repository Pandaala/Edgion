# OpenTelemetry 分布式追踪

> 为 Gateway 接入 OpenTelemetry，支持 W3C TraceContext 传播，可接入 Jaeger/Tempo/Zipkin。
> Priority: P0

## Background

当前只有 tracing crate（本地结构化日志），没有分布式追踪能力。
微服务场景下，跨服务的链路追踪是排查问题的核心工具，缺了会成为生产部署的拦路虎。

## Status

- [ ] 研究阶段（待启动）

## Steps

| Step | Content | Status |
|------|---------|--------|
| step-01 | 现状分析与方案设计 | pending |
| step-02 | 实现 | pending |
| step-03 | 测试 | pending |
