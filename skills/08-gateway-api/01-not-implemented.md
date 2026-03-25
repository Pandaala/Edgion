---
name: gateway-api-not-implemented
description: Gateway API 规范中 Edgion 明确不实现的功能清单及原因
---

# Gateway API — 不实现的功能

本文件记录 Gateway API 规范中 Edgion 经过评估后决定不实现的功能。

## Gateway addresses 实际绑定地址上报

**决定：不实现**

Gateway API 规范期望 `Gateway.status.addresses` 反映数据面实际绑定的地址。Edgion 的 Controller-Gateway 架构使用异步 gRPC 配置同步，无数据面→Controller 状态回传通道，Controller 无法获知 Gateway 实际绑定了哪些地址。

当前行为：从 `spec.addresses`、Service ClusterIP、默认配置、fallback `0.0.0.0` 推导，不反映运行时实际状态。

- 若未来为 Programmed 条件建立状态回传通道，可顺带解决此问题
- 相关合规任务见 [tasks/todo/gateway-api-conformance/03-status-conditions.md](../../tasks/todo/gateway-api-conformance/03-status-conditions.md)
