---
name: edgion-skills
description: Root navigation for the Edgion knowledge base. Read this first, then drill into the relevant subtree.
---

# Edgion Skills

> Rust + Pingora + Gateway API 的 Kubernetes 网关。Controller–Gateway 分离，gRPC 配置同步。
> 支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS、WebSocket，含插件系统、负载均衡、TLS/mTLS。

## 导航规则

1. **渐进式披露**：本文件 → 分类 SKILL.md → 具体文件。只加载当前任务需要的最小子树。
2. **理解 vs 动手**：想理解"X 是什么/怎么工作" → `01-architecture/`；想动手"开发/修改 X" → `02-development/` + `03-coding/`。
3. **跨领域任务**：修改一个资源的 Handler 通常需要 architecture（理解处理流程）→ development（开发规范）→ coding（编码约束）→ testing（验证），按需逐层加载，不要一次全加载。
4. **资源相关任务**：先到 [05-resources/](01-architecture/05-resources/SKILL.md) 找到该资源的架构文档，再根据任务类型决定是否需要 development/testing。

## 快速定位

| 你想了解… | 直接入口 |
|-----------|---------|
| **架构全貌** — Controller/Gateway/Ctl 三 bin 关系 | [01-architecture/SKILL.md](01-architecture/SKILL.md) |
| **Controller 内部** — ConfCenter、Workqueue、ResourceProcessor、Requeue | [01-architecture/01-controller/](01-architecture/01-controller/SKILL.md) |
| **Gateway 内部** — Pingora 生命周期、路由匹配、插件执行、负载均衡 | [01-architecture/02-gateway/](01-architecture/02-gateway/SKILL.md) |
| **Controller↔Gateway 配置同步** — gRPC Watch/List、ConfigSyncServer/Client | [01-architecture/03-controller-gateway-link/](01-architecture/03-controller-gateway-link/SKILL.md) |
| **某种资源的处理链路** — Gateway/HTTPRoute/EdgionPlugins/StreamPlugins/Secret 等 20 种 | [01-architecture/05-resources/](01-architecture/05-resources/SKILL.md) |
| **开发新资源/插件** — 添加 CRD、EdgionPlugin、StreamPlugin、LinkSys | [02-development/SKILL.md](02-development/SKILL.md) |
| **路由匹配规则** — HTTP/gRPC/TCP/TLS/UDP 各引擎的匹配逻辑 | [02-gateway/03-routes/00-route-matching.md](01-architecture/02-gateway/03-routes/00-route-matching.md) |
| **TLS 配置** — 下游 TLS、上游 mTLS、证书管理、SNI 匹配 | [02-gateway/04-tls/00-tls-overview.md](01-architecture/02-gateway/04-tls/00-tls-overview.md) |
| **可观测性** — AccessLog、Metrics、Tracing、日志安全 | [03-coding/SKILL.md](03-coding/SKILL.md) |
| **集成测试** — 架构、运行、新增用例 | [04-testing/01-integration-testing.md](04-testing/01-integration-testing.md) |
| **配置参考** — TOML 配置项、注解、Feature Flags | [02-development/04-config-reference.md](02-development/04-config-reference.md) |
| **ConfHandler 开发** — 分类、增量更新、ArcSwap、配置泄漏防护 | [02-development/08-conf-handler-guidelines.md](02-development/08-conf-handler-guidelines.md) |

## 目录总览

| # | 目录 | 用途 |
|---|------|------|
| 01 | [architecture/](01-architecture/SKILL.md) | 系统架构：通用约定、Controller、Gateway、gRPC 同步、资源、ctl |
| 02 | [development/](02-development/SKILL.md) | 开发指南：新资源、插件开发、配置参考、注解、Feature Flags |
| 03 | [coding/](03-coding/SKILL.md) | 编码规范：日志 ID、日志安全、可观测性（Access Log / Metrics / Tracing） |
| 04 | [testing/](04-testing/SKILL.md) | 测试：单元测试、集成测试、K8s 测试、专项测试 |
| 05 | [debugging/](05-debugging/SKILL.md) | 调试：Admin API、edgion-ctl、常见问题 |
| 06 | [cicd/](06-cicd/SKILL.md) | 构建发布：本地编译、Docker、GitHub Actions |
| 07 | [review/](07-review/SKILL.md) | Review 沉淀：误报判定、可观测性审查 |
| 08 | [gateway-api/](08-gateway-api/SKILL.md) | Gateway API 兼容性备忘 |
| 09 | [task/](09-task/SKILL.md) | 任务管理：目录规则、模板、生命周期阶段、完成检查清单 |
| 10 | [misc/](10-misc/) | 杂项（TLS 排查等） |

## 开发生命周期速查

任务按阶段推进，每阶段加载对应 skills（详见 [09-task/SKILL.md](09-task/SKILL.md)）：

| Phase | 做什么 | 加载 |
|-------|--------|------|
| 1 Analysis | 需求分析、影响评估 | `01-architecture/` |
| 2 Design | 方案设计 | `01-architecture/`, `02-development/` |
| 3 Implementation | 编码 | `02-development/`, `03-coding/` |
| 4 Testing | 测试验证 | `04-testing/` |
| 5 Documentation | 文档更新 | `02-development/07-documentation-writing.md` |
| 6 Review | 回顾、知识回写 | `07-review/` |

## 用户文档

位于 `docs/`，按语言分目录（en、zh-CN、ja）。完整目录树见 [docs/DIRECTORY.md](../docs/DIRECTORY.md)。
