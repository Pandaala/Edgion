---
name: edgion-skills
description: Root navigation for the Edgion knowledge base. Read this first, then drill into the relevant subtree.
---

# Edgion Skills

> Rust + Pingora + Gateway API 的 Kubernetes 网关。Controller–Gateway 分离，gRPC 配置同步。
> 支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS、WebSocket，含插件系统、负载均衡、TLS/mTLS。

## 导航规则

1. **渐进式披露**：本文件 → 分类 SKILL.md → 具体文件。只加载当前任务需要的最小子树。
2. **三层定位**：
   - **理解架构**（内部怎么实现） → `01-architecture/`
   - **查功能/配置**（怎么用/怎么配） → `02-features/`
   - **写代码**（编码规范） → `03-coding/`
3. **跨领域任务**：修改一个资源的 Handler 通常需要 architecture（理解处理流程）→ features（功能/Schema 参考）→ coding（编码约束）→ testing（验证），按需逐层加载。
4. **资源相关任务**——两个目录按编号对齐，职责互补：
   - [01-architecture/05-resources/](01-architecture/05-resources/SKILL.md) — **内部实现**：Handler 流程、requeue 关联、匹配引擎、源码位置
   - [02-features/03-resources/](02-features/03-resources/SKILL.md) — **外部契约**：YAML Schema、字段类型与默认值、配置示例
   - 改代码通常两边都要看：02 查 Schema 确认字段定义，01 查实现确认处理逻辑

## 快速定位

| 你想了解… | 直接入口 |
|-----------|---------|
| **架构全貌** — Controller/Gateway/Ctl 三 bin 关系 | [01-architecture/SKILL.md](01-architecture/SKILL.md) |
| **Controller 内部** — ConfCenter、Workqueue、ResourceProcessor、Requeue | [01-architecture/01-controller/](01-architecture/01-controller/SKILL.md) |
| **Gateway 内部** — Pingora 生命周期、路由匹配、插件执行、负载均衡 | [01-architecture/02-gateway/](01-architecture/02-gateway/SKILL.md) |
| **Controller↔Gateway 配置同步** — gRPC Watch/List | [01-architecture/03-controller-gateway-link/](01-architecture/03-controller-gateway-link/SKILL.md) |
| **某种资源的处理链路** — 20 种资源的架构 | [01-architecture/05-resources/](01-architecture/05-resources/SKILL.md) |
| **二进制启动与部署** — CLI 参数、部署模式 | [02-features/01-binary-and-deployment/](02-features/01-binary-and-deployment/SKILL.md) |
| **TOML 配置 Schema** — Controller/Gateway 配置 | [02-features/02-config/](02-features/02-config/SKILL.md) |
| **资源功能 Schema** — Gateway/Route/TLS/Plugin/Backend/LinkSys | [02-features/03-resources/](02-features/03-resources/SKILL.md) |
| **可观测性** — Access Log、Metrics、协议日志 | [02-features/04-observability/](02-features/04-observability/SKILL.md) |
| **注解参考** — 所有 edgion.io/* 键 | [02-features/05-annotations/](02-features/05-annotations/SKILL.md) |
| **开发新资源/插件** — 添加 CRD、插件、连接器 | [01-architecture/01-controller/09-add-new-resource/](01-architecture/01-controller/09-add-new-resource/00-guide.md) |
| **路由匹配规则** — 各引擎匹配逻辑 | [01-architecture/02-gateway/03-routes/00-route-matching.md](01-architecture/02-gateway/03-routes/00-route-matching.md) |
| **编码规范** — 日志 ID、日志安全、可观测性 | [03-coding/SKILL.md](03-coding/SKILL.md) |
| **集成测试** — 架构、运行、新增用例 | [05-testing/01-integration-testing.md](05-testing/01-integration-testing.md) |

## 目录总览

| # | 目录 | 用途 |
|---|------|------|
| 01 | [architecture/](01-architecture/SKILL.md) | 系统架构 + 开发指南：Controller、Gateway、gRPC 同步、资源处理、插件/资源/连接器开发 |
| 02 | [features/](02-features/SKILL.md) | 功能与配置参考：二进制部署、配置 Schema、资源功能 Schema、可观测性、注解 |
| 03 | [coding/](03-coding/SKILL.md) | 编码规范：日志 ID、日志安全、可观测性（Access Log / Metrics / Tracing） |
| 04 | [review/](04-review/SKILL.md) | Review 沉淀：误报判定、可观测性审查 |
| 05 | [testing/](05-testing/SKILL.md) | 测试：单元测试、集成测试、K8s 测试、专项测试 |
| 06 | [tracing/](06-tracing/SKILL.md) | 调试：Admin API、edgion-ctl、常见问题 |
| 07 | [tasks/](07-tasks/SKILL.md) | 任务管理：目录规则、模板、生命周期阶段、完成检查清单 |
| 08 | [gateway-api/](08-gateway-api/SKILL.md) | Gateway API 兼容性备忘 |
| 09 | [misc/](09-misc/) | 杂项（TLS 排查等） |

## 开发生命周期速查

任务按阶段推进，每阶段加载对应 skills（详见 [07-tasks/SKILL.md](07-tasks/SKILL.md)）：

| Phase | 做什么 | 加载 |
|-------|--------|------|
| 1 Analysis | 需求分析、影响评估 | `01-architecture/` |
| 2 Design | 方案设计、Schema 确认 | `01-architecture/`, `02-features/` |
| 3 Implementation | 编码 | `01-architecture/`（开发指南）, `02-features/`（配置 Schema）, `03-coding/` |
| 4 Testing | 测试验证 | `05-testing/` |
| 5 Review | 代码审查 | `04-review/` |

## 用户文档

位于 `docs/`，按语言分目录（en、zh-CN、ja）。完整目录树见 [docs/DIRECTORY.md](../docs/DIRECTORY.md)。
