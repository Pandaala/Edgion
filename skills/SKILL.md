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

### [architecture/](architecture/SKILL.md) — 架构与核心功能
项目整体架构设计、Controller/Gateway 分离模型、各核心子系统的设计原理。

| 文件 | 主题 |
|------|------|
| [00-overview.md](architecture/00-overview.md) | 项目总览：高层架构图、Crate 结构、代码组织、EdgionHttpContext、edgion-ctl、关键依赖 |
| [01-config-center/](architecture/01-config-center/SKILL.md) | 配置中心：ConfCenter、Workqueue、ResourceProcessor、K8s HA、文件系统模式 |
| [02-grpc-sync.md](architecture/02-grpc-sync.md) | gRPC 配置同步：Proto 定义、同步流程、Server/Client 端实现 |
| [03-data-plane.md](architecture/03-data-plane.md) | 数据面：Gateway 启动流程、Pingora ProxyHttp 生命周期、ConnectionFilter |
| [04-route-matching.md](architecture/04-route-matching.md) | 路由匹配：匹配流水线、RadixPath/Regex 引擎、优先级、多 Gateway 端口共享 |
| [05-plugin-system.md](architecture/05-plugin-system.md) | 插件系统：4 阶段 trait、PluginRuntime、条件执行、预解析机制 |
| [06-load-balancing.md](architecture/06-load-balancing.md) | 负载均衡：EWMA、LeastConn、WeightedSelector、健康检查、后端发现 |
| [07-gateway-api.md](architecture/07-gateway-api.md) | Gateway API：v1.4.0 支持范围、资源映射、一致性测试、Edgion 扩展点 |
| [08-resource-system.md](architecture/08-resource-system.md) | 资源系统：define_resources! 宏、ResourceMeta trait、ResourceKind、Preparse 机制 |
| [09-core-layout.md](architecture/09-core-layout.md) | Core 分层定版：模块放置规范，避免回到旧目录结构 |
| [10-requeue-mechanism.md](architecture/10-requeue-mechanism.md) | Requeue 机制：跨资源依赖、post-init 重校验、handler 注册清单 |

### [development/](development/SKILL.md) — 开发指南
功能开发、插件编写、资源添加、配置参考等开发者日常所需。

| 文件 | 主题 |
|------|------|
| [00-add-new-resource.md](development/00-add-new-resource.md) | 添加新 CRD 资源类型的完整流程 |
| [01-edgion-plugin-dev.md](development/01-edgion-plugin-dev.md) | EdgionPlugin (HTTP 层) 插件开发 |
| [02-stream-plugin-dev.md](development/02-stream-plugin-dev.md) | StreamPlugin (TCP/TLS 两阶段) 插件开发 |
| [03-link-sys-dev.md](development/03-link-sys-dev.md) | LinkSys 外部系统连接器开发 |
| [04-config-reference.md](development/04-config-reference.md) | TOML 配置文件参考 |
| [05-annotations-reference.md](development/05-annotations-reference.md) | `edgion.io/*` 注解参考 |
| [06-feature-flags.md](development/06-feature-flags.md) | Cargo Feature Flags 参考 |
| [07-documentation-writing.md](development/07-documentation-writing.md) | 文档编写规范 |
| [08-conf-handler-guidelines.md](development/08-conf-handler-guidelines.md) | ConfHandler 开发规范：分类、增量更新、ArcSwap、配置泄漏防护 |
| [09-status-management.md](development/09-status-management.md) | Status 管理规范：update_status 实现模式、已知陷阱、审查清单 |

### [observability/](observability/SKILL.md) — 可观测性
Access Log、Metrics、控制面日志的设计原则与操作规范。

| 文件 | 主题 |
|------|------|
| [00-access-log.md](observability/00-access-log.md) | Access Log 设计：字段结构、PluginLog 格式、常见场景、检查清单 |
| [01-metrics.md](observability/01-metrics.md) | Metrics 规范：添加步骤、Label 约束、Test Metrics、禁止事项 |
| [02-tracing-and-logging.md](observability/02-tracing-and-logging.md) | 控制面日志：结构化 Tracing、Level 选择、热路径约束、安全最佳实践 |

### [testing/](testing/SKILL.md) — 测试
集成测试、K8s 测试、LinkSys 测试、调试排错。

| 文件 | 主题 |
|------|------|
| [00-integration-testing.md](testing/00-integration-testing.md) | 集成测试：架构、运行流程、新增测试步骤、调试指南 |
| [01-k8s-integration-testing.md](testing/01-k8s-integration-testing.md) | K8s 集成测试：与本地测试差异、改造清单、执行阶段 |
| [02-link-sys-testing.md](testing/02-link-sys-testing.md) | LinkSys 集成测试：bash 测试流程、Admin API 验证、Docker Compose |
| [03-debugging.md](testing/03-debugging.md) | 调试与排错：本地环境、Admin API、edgion-ctl、常见问题速查 |
| [04-conf-sync-leak-testing.md](testing/04-conf-sync-leak-testing.md) | 配置同步泄漏检测：基础循环测试 + 高级边界场景（wildcard/乱序/并发/orphan 等） |

### [cicd/](cicd/SKILL.md) — CI/CD 与构建
编译、Docker 镜像、GitHub Actions、发布流程。

| 文件 | 主题 |
|------|------|
| [00-local-build.md](cicd/00-local-build.md) | 本地编译：Cargo 命令、Feature 组合、常见编译问题 |
| [01-docker-build.md](cicd/01-docker-build.md) | Docker 编译：多阶段构建、cargo-chef、多架构支持 |
| [02-github-workflow.md](cicd/02-github-workflow.md) | GitHub Workflow：CI 流水线、共享 setup-rust、本地 action、Release 发布、镜像推送 |

### [coding-standards/](coding-standards/SKILL.md) — 编码规范
日志 ID 传播、敏感信息防泄漏、控制面/数据面日志分离等通用编码规范。

| 文件 | 主题 |
|------|------|
| [00-logging-and-tracing-ids.md](coding-standards/00-logging-and-tracing-ids.md) | 日志 ID 传播：rv / sv / key_name 三元组，确保控制面→数据面可关联 |
| [01-log-safety.md](coding-standards/01-log-safety.md) | 日志安全：敏感信息不入日志、配置不泄漏、数据面禁用 tracing |

### [review/](review/SKILL.md) — Review 知识沉淀
代码审查中的项目特定结论、常见误报、可直接复用的判定标准。

| 文件 | 主题 |
|------|------|
| [SKILL.md](review/SKILL.md) | Review 目录总览与使用方式 |
| [memory-leak/not-a-memory-leak.md](review/memory-leak/not-a-memory-leak.md) | 非内存泄漏场景判定，避免重复误报 |

### [task/](task/SKILL.md) — 任务记录规范
任务如何在 `tasks/` 下组织、拆 step、记录状态，以及各阶段如何关联到对应的 skills 知识。

| 文件 | 主题 |
|------|------|
| [SKILL.md](task/SKILL.md) | 任务流程规范：目录规则、step 命名、状态约定、各阶段 skills 关联、完成后检查清单 |

### [gateway-api/](gateway-api/SKILL.md) — Gateway API 兼容性备忘
Gateway API 实现中的有意偏差和边界决策。

| 文件 | 主题 |
|------|------|
| [SKILL.md](gateway-api/SKILL.md) | TLS 证书选择策略：不支持 hostname-less catch-all、不支持 cross-port fallback |

### misc [mis/](mis/) — 杂项知识
不属于上述分类的诊断指南和临时记录。

| 文件 | 主题 |
|------|------|
| [debugging-tls-gateway.md](mis/debugging-tls-gateway.md) | TLS Gateway 路由问题排查流程 |

## 用户文档

用户文档位于 `docs/` 目录，按语言分目录（en、zh-CN、ja）。
完整目录树见 [docs/DIRECTORY.md](../docs/DIRECTORY.md)。
