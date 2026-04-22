---
name: redis-fail-open
description: Redis 限流 fail-open 默认行为类 finding 判定规则。审查"Redis 故障时限流被完全绕过"类 finding 时先读此文件。
---

# Redis 限流 fail-open — NOT-AN-ISSUE

## 结论

`onRedisFailure` 默认为 `Allow`（fail-open）判定为 NOT-AN-ISSUE。不接受"改默认为 `Deny`"或"fail-open 是安全漏洞"类修复建议。

## 核心理由

**1. 配置项已存在，operator 可自主选择**

`onRedisFailure: Deny` 选项已实现且有文档说明。需要 fail-close 的高安全场景，operator 显式配置即可。这不是缺失功能，是已提供的选择项。

**2. fail-open vs fail-close 是运维权衡，没有唯一正确答案**

- **fail-open（默认）**：Redis 故障时服务继续，限流暂时失效。对大多数业务场景，"服务可用但无限流"优于"服务不可用"
- **fail-close**：Redis 故障时拒绝所有受限流保护的请求，相当于主动造成服务中断

**3. 行业一致**

Kong Redis 限流插件、APISIX Redis 限流插件均默认 fail-open，理由相同：Redis 是可选的辅助组件，其故障不应级联成服务中断。

**4. 攻击者触发 Redis 故障的门槛极高**

攻击者若能触发 Redis 集群故障（通常是强大的 DDoS 能力），已经可以直接打 Gateway，绕过限流是次要威胁。

## 不接受的修复建议

- 将默认值改为 `Deny`（会在 Redis 故障时意外中断服务，影响所有依赖限流的路由）
- 将 fail-open 标为"安全缺陷"（这是有文档说明的设计取舍）
- 强制要求 operator 显式配置 `onRedisFailure`（增加配置负担，对大多数场景没有价值）
