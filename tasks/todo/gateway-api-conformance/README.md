# Gateway API 合规性优化计划

基于 Kubernetes Gateway API v1.4 规范与 Edgion 当前实现的深度对比分析。

## 文档索引

| 文件 | 内容 | 优先级 |
|------|------|--------|
| [01-httproute-retry-session.md](01-httproute-retry-session.md) | HTTPRoute Retry 策略运行时接入 + Session Persistence 实现 | P0 |
| [02-tls-passthrough.md](02-tls-passthrough.md) | TLS Passthrough 模式实现 | P0 |
| [03-status-conditions.md](03-status-conditions.md) | Programmed 条件、supportedFeatures、PartiallyInvalid 等状态上报 | P0/P1 |
| [04-grpcroute-gaps.md](04-grpcroute-gaps.md) | GRPCRoute 正则匹配 + UNIMPLEMENTED 错误码 | P1 |
| [05-filter-validation.md](05-filter-validation.md) | Filter 冲突校验 + 版本更新 + 约束执行 | P1 |
| [06-policy-and-experimental.md](06-policy-and-experimental.md) | BackendTrafficPolicy、externalAuth、infrastructure 等前瞻性功能 | P2 |
| [07-code-quality.md](07-code-quality.md) | 性能优化与代码质量改进 | P2 |

## 优先级定义

- **P0**: 核心功能缺失或 GA 规范不符合，应尽快修复
- **P1**: 合规性/完整性问题，影响用户体验
- **P2**: 前瞻性功能或代码质量优化
