# 熔断器（Circuit Breaker）

> 为 Gateway 添加熔断器机制，防止级联故障，保护上游服务。
> Priority: P1

## Background

当前健康检查是被动剔除，没有主动熔断。上游持续失败时，请求仍会不断打过去，造成级联故障。
Kong/APISIX/Envoy 都把熔断作为核心功能，缺了会被用户直接比较。

标准熔断器状态机：Closed → Open（连续失败 N 次）→ Half-Open（探测）→ Closed/Open

## Status

- [ ] 研究阶段（待启动）

## Steps

| Step | Content | Status |
|------|---------|--------|
| step-01 | 现状分析与方案设计 | pending |
| step-02 | 实现 | pending |
| step-03 | 测试 | pending |
