---
name: edgion-skills
description: Root navigation skill for the Edgion repository. Use this file first when you need project-specific knowledge, then follow it into the smallest relevant architecture, development, testing, observability, review, or task-tracking subtree.
---

# Edgion Skills — 项目知识库总目录

> 基于 Rust + Pingora + Gateway API 的 Kubernetes 网关。Controller–Gateway 分离架构，通过 gRPC 做配置同步。
> 支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS、WebSocket，具备插件系统、多种负载均衡策略和 TLS/mTLS 能力。

## 使用方式

本知识库采用**渐进式披露**（Progressive Disclosure）组织：
1. **本文件**是总入口，AI 首次阅读时只需读取本文件
2. **分类 SKILL.md** 提供该领域的概述和文件清单，按需深入
3. **具体文件** 包含完整的技术细节，仅在需要时加载

## 知识库目录

### [00-lifecycle/](00-lifecycle/SKILL.md) — 开发生命周期（任务起点）
每个任务的**第一个入口**。定义了从需求到上线的 6 个阶段、各阶段的门禁标准、AI 在每个阶段应加载的 skills 映射。

| 文件 | 主题 |
|------|------|
| [SKILL.md](00-lifecycle/SKILL.md) | 生命周期 6 阶段定义、阶段→Skills 映射、门禁标准、裁剪规则 |
| [01-task-template.md](00-lifecycle/01-task-template.md) | 任务主文件模板、Step 文件命名规范、创建/推进/关闭流程 |
| [02-skills-directory-design.md](00-lifecycle/02-skills-directory-design.md) | Skills 目录体系设计：编号规范、依赖方向、新增/重命名规则 |

### [01-architecture/](01-architecture/SKILL.md) — 架构与核心功能
按 bin 类型和资源类型组织的架构文档，涵盖通用约定、各 bin 架构、gRPC 同步、资源处理。

| 子目录 | 主题 |
|--------|------|
| [00-common/](01-architecture/00-common/SKILL.md) | 通用约定：项目总览、命令行/目录/配置、Core 分层、资源系统 |
| [01-controller/](01-architecture/01-controller/SKILL.md) | Controller 架构：启动/关闭、Admin API、配置中心、Workqueue、Requeue |
| [02-gateway/](01-architecture/02-gateway/SKILL.md) | Gateway 架构：Pingora 生命周期、路由、TLS、插件、负载均衡、后端 |
| [03-controller-gateway-link/](01-architecture/03-controller-gateway-link/SKILL.md) | Controller↔Gateway gRPC 双向同步 |
| [04-ctl/](01-architecture/04-ctl/SKILL.md) | edgion-ctl CLI 工具架构 |
| [05-resources/](01-architecture/05-resources/SKILL.md) | 资源架构：通用流程 + 每种资源的功能点/关联 |
| [06-gateway-api.md](01-architecture/06-gateway-api.md) | Gateway API v1.4.0 合规性 |

### [02-development/](02-development/SKILL.md) — 开发指南
功能开发、插件编写、资源添加、配置参考等开发者日常所需。

| 文件 | 主题 |
|------|------|
| [00-add-new-resource.md](02-development/00-add-new-resource.md) | 添加新 CRD 资源类型的完整流程 |
| [01-edgion-plugin-dev.md](02-development/01-edgion-plugin-dev.md) | EdgionPlugin (HTTP 层) 插件开发 |
| [02-stream-plugin-dev.md](02-development/02-stream-plugin-dev.md) | StreamPlugin (TCP/TLS 两阶段) 插件开发 |
| [03-link-sys-dev.md](02-development/03-link-sys-dev.md) | LinkSys 外部系统连接器开发 |
| [04-config-reference.md](02-development/04-config-reference.md) | TOML 配置文件参考 |
| [05-annotations-reference.md](02-development/05-annotations-reference.md) | `edgion.io/*` 注解参考 |
| [06-feature-flags.md](02-development/06-feature-flags.md) | Cargo Feature Flags 参考 |
| [07-documentation-writing.md](02-development/07-documentation-writing.md) | 文档编写规范 |
| [08-conf-handler-guidelines.md](02-development/08-conf-handler-guidelines.md) | ConfHandler 开发规范：分类、增量更新、ArcSwap、配置泄漏防护 |
| [09-status-management.md](02-development/09-status-management.md) | Status 管理规范：update_status 实现模式、已知陷阱、审查清单 |

### [03-coding/](03-coding/SKILL.md) — 编码规范与可观测性
日志 ID 传播、敏感信息防泄漏、控制面/数据面日志分离、Access Log / Metrics / 控制面 Tracing 设计规范。

| 文件 | 主题 |
|------|------|
| [00-logging-and-tracing-ids.md](03-coding/00-logging-and-tracing-ids.md) | 日志 ID 传播：rv / sv / key_name 三元组，确保控制面→数据面可关联 |
| [01-log-safety.md](03-coding/01-log-safety.md) | 日志安全：敏感信息不入日志、配置不泄漏、数据面禁用 tracing |
| [observability/00-access-log.md](03-coding/observability/00-access-log.md) | Access Log 设计：字段结构、PluginLog 格式、常见场景速查 |
| [observability/01-metrics.md](03-coding/observability/01-metrics.md) | Metrics 规范：添加步骤、Label 约束、Test Metrics、禁止事项 |
| [observability/02-tracing-and-logging.md](03-coding/observability/02-tracing-and-logging.md) | 控制面日志：结构化 Tracing、Level 选择、错误上下文、instrument 命名 |

### [04-testing/](04-testing/SKILL.md) — 测试
单元测试 + 集成测试（目标 99% 覆盖率），以及专项测试。

| 文件 | 主题 |
|------|------|
| **标准测试** | |
| [00-unit-testing.md](04-testing/00-unit-testing.md) | 单元测试：inline test module、运行方式、编写规范、与集成测试的分工 |
| [01-integration-testing.md](04-testing/01-integration-testing.md) | 集成测试：架构、运行流程、新增测试步骤 |
| [02-k8s-integration-testing.md](04-testing/02-k8s-integration-testing.md) | K8s 集成测试：与本地测试差异、改造清单、执行阶段 |
| **专项测试** | |
| [03-link-sys-testing.md](04-testing/03-link-sys-testing.md) | LinkSys 集成测试：bash 测试流程、Admin API 验证、Docker Compose |
| [04-conf-sync-leak-testing.md](04-testing/04-conf-sync-leak-testing.md) | 配置同步泄漏检测：基础循环测试 + 高级边界场景（wildcard/乱序/并发/orphan 等） |

### [05-debugging/](05-debugging/SKILL.md) — 调试与排障
日常开发排障：Admin API、edgion-ctl 三视图、Access Log Store、Metrics。

| 文件 | 主题 |
|------|------|
| [00-debugging.md](05-debugging/00-debugging.md) | 调试与排错：本地环境、Admin API、edgion-ctl、常见问题速查 |

### [06-cicd/](06-cicd/SKILL.md) — CI/CD 与构建
编译、Docker 镜像、GitHub Actions、发布流程。

| 文件 | 主题 |
|------|------|
| [00-local-build.md](06-cicd/00-local-build.md) | 本地编译：Cargo 命令、Feature 组合、常见编译问题 |
| [01-docker-build.md](06-cicd/01-docker-build.md) | Docker 编译：多阶段构建、cargo-chef、多架构支持 |
| [02-github-workflow.md](06-cicd/02-github-workflow.md) | GitHub Workflow：CI 流水线、共享 setup-rust、本地 action、Release 发布、镜像推送 |

### [07-review/](07-review/SKILL.md) — Review 知识沉淀
代码审查中的项目特定结论、常见误报、可观测性审查、可直接复用的判定标准。

| 文件 | 主题 |
|------|------|
| [SKILL.md](07-review/SKILL.md) | Review 目录总览与使用方式 |
| [memory-leak/not-a-memory-leak.md](07-review/memory-leak/not-a-memory-leak.md) | 非内存泄漏场景判定，避免重复误报 |
| [observability/observability-review-rule.md](07-review/observability/observability-review-rule.md) | 可观测性审查：数据面零 tracing、metrics 爆炸防护、access log 质量 |

### [08-gateway-api/](08-gateway-api/SKILL.md) — Gateway API 兼容性备忘
Gateway API 实现中的有意偏差和边界决策。

| 文件 | 主题 |
|------|------|
| [SKILL.md](08-gateway-api/SKILL.md) | TLS 证书选择策略：不支持 hostname-less catch-all、不支持 cross-port fallback |

### [09-task/](09-task/SKILL.md) — 任务记录规范
任务如何在 `tasks/` 下组织、拆 step、记录状态，以及各阶段如何关联到对应的 skills 知识。

| 文件 | 主题 |
|------|------|
| [SKILL.md](09-task/SKILL.md) | 任务流程规范：目录规则、step 命名、状态约定、各阶段 skills 关联、完成后检查清单 |

### [10-misc/](10-misc/) — 杂项知识
不属于上述分类的诊断指南和临时记录。

| 文件 | 主题 |
|------|------|
| [debugging-tls-gateway.md](10-misc/debugging-tls-gateway.md) | TLS Gateway 路由问题排查流程 |

## 用户文档

用户文档位于 `docs/` 目录，按语言分目录（en、zh-CN、ja）。
完整目录树见 [docs/DIRECTORY.md](../docs/DIRECTORY.md)。
