# Gateway API 差距分析 — 待修复/待支持清单

来源：2026-03-27 全量对比 Gateway API v1.5 规范 vs Edgion 当前实现。

## 总览

| # | 文件 | 资源 | 类型 | 优先级 | 工作量 |
|---|------|------|------|--------|--------|
| 01 | [01-referencegrant-upgrade-v1.md](01-referencegrant-upgrade-v1.md) | ReferenceGrant | 版本升级 | P0 | 小 |
| 02 | [02-grpcroute-rule-name.md](02-grpcroute-rule-name.md) | GRPCRoute | 字段缺失 | P0 | 小 |
| 03 | [03-httproute-session-persistence.md](03-httproute-session-persistence.md) | HTTPRoute | 功能缺失 | P1 | 中 |
| 04 | [04-backend-tls-san-validation.md](04-backend-tls-san-validation.md) | BackendTLSPolicy | 功能缺失 | P1 | 中 |
| 05 | [05-backend-tls-wellknown-ca.md](05-backend-tls-wellknown-ca.md) | BackendTLSPolicy | 功能缺失 | P2 | 小 |
| 06 | [06-tlsroute-terminate-conformance.md](06-tlsroute-terminate-conformance.md) | TLSRoute | 一致性修复 | P1 | 中 |
| 07 | [07-backend-tls-single-targetref.md](07-backend-tls-single-targetref.md) | BackendTLSPolicy | 规范对齐 | P2 | 小 |

## 优先级定义

- **P0**: 工作量小且影响一致性声明，应立即修复
- **P1**: 功能性缺失，影响用户场景或一致性测试
- **P2**: 规范对齐，当前不影响功能但影响合规性

## 使用方式

每个文件包含完整的问题描述、参考 skill、涉及文件和实现建议。可启动 subagent 逐个修复：

```
# 示例：用 subagent 修复某个具体问题
# 读取对应文件获取上下文，然后执行修复
```
