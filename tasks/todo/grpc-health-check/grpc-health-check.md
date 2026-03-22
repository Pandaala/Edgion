# gRPC 健康检查协议

> 为健康检查系统添加 gRPC Health Check Protocol 支持（grpc.health.v1.Health）。
> Priority: P2

## Background

当前健康检查只支持 HTTP/TCP 探针。K8s 环境中 gRPC 服务广泛使用标准 gRPC 健康检查协议，
不支持会导致 gRPC 后端的健康检查不准确，只能退化为 TCP 连通性检查。

## Status

- [ ] 研究阶段（待启动）

## Steps

| Step | Content | Status |
|------|---------|--------|
| step-01 | 现状分析与方案设计 | pending |
| step-02 | 实现 | pending |
| step-03 | 测试 | pending |
