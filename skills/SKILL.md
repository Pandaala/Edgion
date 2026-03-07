# Edgion Skills — 项目知识库总目录

> 基于 Rust + Pingora + Gateway API 的 Kubernetes 网关。Controller–Gateway 分离架构，通过 gRPC 做配置同步。
> 支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS、WebSocket，具备插件系统、多种负载均衡策略和 TLS/mTLS 能力。

## 使用方式

本知识库采用**渐进式披露**（Progressive Disclosure）组织：
1. **本文件**是总入口，AI 首次阅读时只需读取本文件
2. **分类 SKILL.md** 提供该领域的概述和文件清单，按需深入
3. **具体文件** 包含完整的技术细节，仅在需要时加载

## 知识库目录

### 00 [00-architecture/](00-architecture/SKILL.md) — 架构与核心功能
项目整体架构设计、Controller/Gateway 分离模型、各核心子系统的设计原理。

| 文件 | 主题 |
|------|------|
| [00-overview.md](00-architecture/00-overview.md) | 项目总览：高层架构图、Crate 结构、代码组织、EdgionHttpContext、edgion-ctl、关键依赖 |
| [01-config-center.md](00-architecture/01-config-center.md) | 配置中心：ConfCenter、Workqueue、ResourceProcessor、跨资源 Requeue、DAG 约束 |
| [02-grpc-sync.md](00-architecture/02-grpc-sync.md) | gRPC 配置同步：Proto 定义、同步流程、Server/Client 端实现 |
| [03-data-plane.md](00-architecture/03-data-plane.md) | 数据面：Gateway 启动流程、Pingora ProxyHttp 生命周期、ConnectionFilter |
| [04-route-matching.md](00-architecture/04-route-matching.md) | 路由匹配：匹配流水线、RadixPath/Regex 引擎、优先级、多 Gateway 端口共享 |
| [05-plugin-system.md](00-architecture/05-plugin-system.md) | 插件系统：4 阶段 trait、PluginRuntime、条件执行、预解析机制 |
| [06-load-balancing.md](00-architecture/06-load-balancing.md) | 负载均衡：EWMA、LeastConn、WeightedSelector、健康检查、后端发现 |
| [07-gateway-api.md](00-architecture/07-gateway-api.md) | Gateway API：v1.4.0 支持范围、资源映射、一致性测试、Edgion 扩展点 |
| [08-resource-system.md](00-architecture/08-resource-system.md) | 资源系统：define_resources! 宏、ResourceMeta trait、ResourceKind、Preparse 机制 |

### 01 [01-development/](01-development/SKILL.md) — 开发指南
功能开发、插件编写、资源添加、配置参考等开发者日常所需。

| 文件 | 主题 |
|------|------|
| [00-add-new-resource.md](01-development/00-add-new-resource.md) | 添加新 CRD 资源类型的完整流程 |
| [01-edgion-plugin-dev.md](01-development/01-edgion-plugin-dev.md) | EdgionPlugin (HTTP 层) 插件开发 |
| [02-stream-plugin-dev.md](01-development/02-stream-plugin-dev.md) | StreamPlugin (TCP 层) 插件开发 |
| [03-link-sys-dev.md](01-development/03-link-sys-dev.md) | LinkSys 外部系统连接器开发 |
| [04-config-reference.md](01-development/04-config-reference.md) | TOML 配置文件参考 |
| [05-annotations-reference.md](01-development/05-annotations-reference.md) | `edgion.io/*` 注解参考 |
| [06-feature-flags.md](01-development/06-feature-flags.md) | Cargo Feature Flags 参考 |
| [07-documentation-writing.md](01-development/07-documentation-writing.md) | 文档编写规范 |

### 02 [02-observability/](02-observability/SKILL.md) — 可观测性
Access Log、Metrics、控制面日志的设计原则与操作规范。

| 文件 | 主题 |
|------|------|
| [00-access-log.md](02-observability/00-access-log.md) | Access Log 设计：字段结构、PluginLog 格式、常见场景、检查清单 |
| [01-metrics.md](02-observability/01-metrics.md) | Metrics 规范：添加步骤、Label 约束、Test Metrics、禁止事项 |
| [02-tracing-and-logging.md](02-observability/02-tracing-and-logging.md) | 控制面日志：结构化 Tracing、Level 选择、热路径约束、安全最佳实践 |

### 03 [03-testing/](03-testing/SKILL.md) — 测试
集成测试、K8s 测试、LinkSys 测试、调试排错。

| 文件 | 主题 |
|------|------|
| [00-integration-testing.md](03-testing/00-integration-testing.md) | 集成测试：架构、运行流程、新增测试步骤、调试指南 |
| [01-k8s-integration-testing.md](03-testing/01-k8s-integration-testing.md) | K8s 集成测试：与本地测试差异、改造清单、执行阶段 |
| [02-link-sys-testing.md](03-testing/02-link-sys-testing.md) | LinkSys 集成测试：bash 测试流程、Admin API 验证、Docker Compose |
| [03-debugging.md](03-testing/03-debugging.md) | 调试与排错：本地环境、Admin API、edgion-ctl、常见问题速查 |

### 04 [04-cicd/](04-cicd/SKILL.md) — CI/CD 与构建
编译、Docker 镜像、GitHub Actions、发布流程。

| 文件 | 主题 |
|------|------|
| [00-local-build.md](04-cicd/00-local-build.md) | 本地编译：Cargo 命令、Feature 组合、常见编译问题 |
| [01-docker-build.md](04-cicd/01-docker-build.md) | Docker 编译：多阶段构建、cargo-chef、多架构支持 |
| [02-github-workflow.md](04-cicd/02-github-workflow.md) | GitHub Workflow：CI 流水线、Release 发布、镜像推送 |
