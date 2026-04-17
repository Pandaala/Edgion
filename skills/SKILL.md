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
| **系统架构** — Controller/Gateway/Ctl 三 bin 关系、内部模块、开发指南 | [01-architecture/SKILL.md](01-architecture/SKILL.md) |
| **二进制启动与部署** — CLI 参数、部署模式 | [02-features/01-binary-and-deployment/](02-features/01-binary-and-deployment/SKILL.md) |
| **TOML 配置 Schema** — Controller/Gateway 配置 | [02-features/02-config/](02-features/02-config/SKILL.md) |
| **资源功能 Schema** — Gateway/Route/TLS/Plugin/Backend/LinkSys | [02-features/03-resources/](02-features/03-resources/SKILL.md) |
| **可观测性** — Access Log、Metrics、协议日志 | [02-features/04-observability/](02-features/04-observability/SKILL.md) |
| **注解参考** — 所有 edgion.io/* 键 | [02-features/05-annotations/](02-features/05-annotations/SKILL.md) |
| **编码规范** — 日志 ID、日志安全、可观测性 | [03-coding/SKILL.md](03-coding/SKILL.md) |
| **集成测试** — 架构、运行、新增用例 | [05-testing/01-integration-testing.md](05-testing/01-integration-testing.md) |
| **全局 Review** — 安全/正确性/可观测性检查清单 + 已关闭误报条目 | [10-fully-review/SKILL.md](10-fully-review/SKILL.md) |
| **用户文档规范** — 撰写/审阅 docs/ 文档的写作规范 | [11-doc/SKILL.md](11-doc/SKILL.md) |
| **CI/发布自动化** — GitHub Release、tag 推送、Docker 镜像、发布说明文件位置 | [09-misc/cicd/02-github-workflow.md](09-misc/cicd/02-github-workflow.md) |
| **Center 联邦持久化** — SQLite `controllers` 表、CenterDb、database 配置（PM 数据已改用反向 Watch，无 SQLite 缓存） | [09-misc/center-db.md](09-misc/center-db.md) |
| **Center 反向 Watch 架构** — CenterWatchCache、CenterMetaDataStore、federation gRPC 双向流 PM 数据流 | [09-misc/center-db.md#pluginmetadata-数据流反向-watch-架构](09-misc/center-db.md) |
| **开发部署热替换** — 原生镜像 + shell loop + 交叉编译 + kubectl cp | [09-misc/03-dev-deploy-hot-replace.md](09-misc/03-dev-deploy-hot-replace.md) |

> 架构内部细粒度定位（Controller/Gateway/Link/资源/插件开发/路由匹配等）见 [01-architecture/SKILL.md](01-architecture/SKILL.md)。

## 关键约束（必读，违反会引入运行时 bug）

> 这里只列"非显而易见、代码本身看不出来、犯过错"的约束。

| 约束 | 位置 | 详情 |
|------|------|------|
| **PluginConditions regex 必须在 preparse compile** | `conditions/types.rs`, `edgion_plugins/mod.rs` | `compiled_regex` 是 `#[serde(skip)]`，反序列化后为 None。必须在 `EdgionPlugins::preparse()` 阶段调用 `compile_conditions()` 完成编译，否则每请求触发 `Regex::new()`。新增带 regex 的条件类型时，同步在 `Condition::compile()` 加 match 臂。详见 [05-plugin-system.md §条件执行](01-architecture/02-gateway/05-plugin-system.md) |
| **gRPC H2 grpc-status 必须放 trailers** | Pingora H2 响应路径 | 放在 response headers 里 tonic 会返回 Internal(13)，必须放 trailers |
| **数据面请求/连接热路径禁止 tracing** | `skills/03-coding/01-log-safety.md` 铁律 3 | 请求/连接处理路径只能用 access log / `PluginLog` / `ctx.err_log` / Metrics，任何 `tracing::` 宏（含 `debug!`）都禁止。例外：配置加载路径、进程启动/关闭、`pg_logging.rs` access log 发送失败。插件 `run_request` / `run_upstream_response` / TLS proxy / stream proxy 全部适用。 |

## 目录总览

| # | 目录 | 用途 |
|---|------|------|
| 01 | [architecture/](01-architecture/SKILL.md) | 系统架构 + 开发指南：Controller、Gateway、gRPC 同步、资源处理、插件/资源/连接器开发 |
| 02 | [features/](02-features/SKILL.md) | 功能与配置参考：二进制部署、配置 Schema、资源功能 Schema、可观测性、注解 |
| 03 | [coding/](03-coding/SKILL.md) | 编码规范：日志 ID、日志安全、可观测性（Access Log / Metrics / Tracing） |
| 04 | [review/](04-review/SKILL.md) | Review 沉淀：误报判定、可观测性审查 |
| 05 | [testing/](05-testing/SKILL.md) | 测试：单元测试、集成测试、K8s 测试、专项测试 |
| 06 | [tracing/](06-tracing/SKILL.md) | 调试：Admin API、edgion-cli、常见问题 |
| 07 | [tasks/](07-tasks/SKILL.md) | 任务管理：目录规则、模板、生命周期阶段、完成检查清单 |
| 08 | [kubernetes/](08-kubernetes/SKILL.md) | Kubernetes 相关：Gateway API 兼容性、API 版本升级策略 |
| 09 | [misc/](09-misc/) | 杂项（TLS 排查等） |
| 10 | [fully-review/](10-fully-review/SKILL.md) | 全局 Review 指南：各维度检查清单、安全误判规则、已关闭条目 |
| 11 | [11-doc/](11-doc/SKILL.md) | 用户文档规范：目录结构、写作风格、页面模板、多语言、版本管理、Review 检查清单 |

## 开发生命周期速查

任务按阶段推进，每阶段加载对应 skills（详见 [07-tasks/SKILL.md](07-tasks/SKILL.md)）：

| Phase | 做什么 | 加载 |
|-------|--------|------|
| 1 Analysis | 需求分析、影响评估 | `01-architecture/` |
| 2 Design | 方案设计、Schema 确认 | `01-architecture/`, `02-features/` |
| 3 Implementation | 编码 | `01-architecture/`（开发指南）, `02-features/`（配置 Schema）, `03-coding/` |
| 4 Testing | 测试验证 | `05-testing/` |
| 5 Review | 代码审查 | `04-review/`, `10-fully-review/`（全局 review 或评审 AI 报告时） |

## 用户文档

位于 `docs/`，按语言分目录（en、zh-CN、ja）。完整目录树见 [docs/DIRECTORY.md](../docs/DIRECTORY.md)。

撰写或审阅用户文档时，遵循 [11-doc/SKILL.md](11-doc/SKILL.md) 中的写作规范。
