# Gateway API 合规性优化计划 — **已关闭**

基于 Kubernetes Gateway API v1.4 规范与 Edgion 当前实现的深度对比分析。

> **状态**：所有项目已完成、标记不实现或移交。ConformanceProfiles GRPC/TLS 验证工作移至 `tasks/todo/gateway-api-1.5-conformance-tests/`。

## 文档索引

| 文件 | 内容 | 状态 |
|------|------|------|
| ~~01-httproute-retry-session.md~~ | ~~HTTPRoute Retry 策略运行时接入~~ | **已完成** |
| ~~02-tls-passthrough.md~~ | ~~TLS Passthrough 模式实现~~ | **已完成** |
| [03-status-conditions.md](03-status-conditions.md) | ~~supportedFeatures~~ ✅ / ~~PartiallyInvalid~~ ✅ / ~~Programmed~~ 不实现 / ~~Gateway addresses~~ 不实现 | **全部关闭** |
| [04-grpcroute-gaps.md](04-grpcroute-gaps.md) | ~~GRPCRoute 正则匹配~~ ✅ / ~~UNIMPLEMENTED 错误码~~ ✅ / ~~Hostname 唯一性~~ 不实现 | **全部关闭** |
| [05-filter-validation.md](05-filter-validation.md) | ~~URLRewrite+Redirect 冲突~~ ✅ / ~~超时约束~~ ✅ / ~~TCP/UDP 单路由~~ ✅ / ~~parametersRef~~ ✅ | **全部完成** |
| [06-policy-and-experimental.md](06-policy-and-experimental.md) | ~~ExternalAuth~~ ✅ / ~~frontendValidation~~ ✅ / ~~ConformanceProfiles (HTTP)~~ ✅ / infrastructure 不实现 / BTP 不实现 / ConformanceProfiles (GRPC/TLS) → v1.5 | **全部关闭** |
| [07-code-quality.md](07-code-quality.md) | ~~GRPCRoute clone~~ ✅ / ~~TLS duplex~~ 不改 / ~~weighted_selector~~ 不改 / ~~target key~~ 已修注释 | **全部关闭** |
