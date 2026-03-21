# 知识来源映射与维护规则

本文档定义 Edgion 仓库里 `AGENTS.md`、`skills/`、`docs/` 和平台专属薄封装的分工，避免同一份知识在多个地方失控复制。

## 分层原则

- `AGENTS.md`：跨平台共享入口，放仓库级规则、最常用命令和导航方式。
- `skills/`：面向 Agent 的任务型知识层，优先承载高频 workflow、排障路径和项目特有约束。
- `docs/`：面向人的长篇说明、多语言开发文档、设计说明和背景材料。
- 平台专属文件：只做薄封装，例如 `CLAUDE.md`、`.cursor/rules/00-edgion-entry.mdc`，不维护第二套知识库。

## 当前主题映射

| 主题 | 给人看的权威位置 | 给 Agent 的入口 | 维护规则 |
|------|------------------|----------------|----------|
| AI 协作方式 | [ai-agent-collaboration.md](./ai-agent-collaboration.md) | [AGENTS.md](../../../AGENTS.md)、[skills/SKILL.md](../../../skills/SKILL.md) | 协作流程写在 `AGENTS.md` 和本文档，不把平台说明分散到多个专属文件 |
| 项目整体架构 | [architecture-overview.md](./architecture-overview.md) | [skills/01-architecture/SKILL.md](../../../skills/01-architecture/SKILL.md)、[00-project-overview.md](../../../skills/01-architecture/00-common/00-project-overview.md) | `docs/` 讲背景和全局视角，`skills/` 讲 AI 该先读哪里 |
| 资源架构 / 注册体系 | [resource-architecture-overview.md](./resource-architecture-overview.md)、[resource-registry-guide.md](./resource-registry-guide.md) | [03-resource-system.md](../../../skills/01-architecture/00-common/03-resource-system.md) | 资源系统的实现细节优先维护在 `skills/`，`docs/` 保留人类友好的讲解 |
| 添加新资源类型 | [add-new-resource-guide.md](./add-new-resource-guide.md) | [00-add-new-resource.md](../../../skills/02-development/00-add-new-resource.md)、[01-integration-testing.md](../../../skills/04-testing/01-integration-testing.md) | Agent 侧保留可执行 workflow，人类侧保留背景、原则和完整示例 |
| HTTP 插件开发 | [http-plugin-development.md](./http-plugin-development.md) | [01-edgion-plugin-dev.md](../../../skills/02-development/01-edgion-plugin-dev.md) | `docs/` 讲执行阶段和实现边界，`skills/` 讲新增插件的落地步骤；终端用户如何配置仍放在 user-guide |
| Stream plugin 开发 | [stream-plugin-development.md](./stream-plugin-development.md) | [02-stream-plugin-dev.md](../../../skills/02-development/02-stream-plugin-dev.md) | `docs/` 讲实现背景和边界，`skills/` 讲可执行 workflow；用户如何配置仍放在 user-guide |
| 运行时配置 / 路径行为 | [work-directory.md](./work-directory.md) | [04-config-reference.md](../../../skills/02-development/04-config-reference.md) | 配置分层、路径选择和进程级配置入口优先放在 `skills/`，长篇背景说明保留在 `docs/` |
| 注解机制 | [annotations-guide.md](./annotations-guide.md) | [05-annotations-reference.md](../../../skills/02-development/05-annotations-reference.md) | `docs/` 讲放置位置和实现边界，详细键表与保留键清单维护在 `skills/reference` |
| 日志 / 可观测性 | [logging-system.md](./logging-system.md) | [skills/03-coding/SKILL.md](../../../skills/03-coding/SKILL.md)、[00-access-log.md](../../../skills/03-coding/observability/00-access-log.md)、[02-tracing-and-logging.md](../../../skills/03-coding/observability/02-tracing-and-logging.md) | `docs/` 负责系统设计讲解，`skills/` 负责落地规则和改动检查点 |
| CI / Release / 镜像发布 | [ci-release-workflow.md](./ci-release-workflow.md) | [skills/06-cicd/SKILL.md](../../../skills/06-cicd/SKILL.md)、[02-github-workflow.md](../../../skills/06-cicd/02-github-workflow.md) | `docs/` 讲发布流程和人工审查点，`skills/` 讲具体 workflow 与命令 |
| Work directory | [work-directory.md](./work-directory.md) | 暂无独立 skill，必要时直接读文档 | 如果未来这块成为高频开发任务，再抽成 skill；在此之前保持 docs-first |
| JWT Auth 设计稿 | [jwt-auth-plugin-design.md](./jwt-auth-plugin-design.md) | 仅在相关任务中按需读取 | 设计评审记录保留在 `docs/`，不要强行转成通用 skill |
| Requeue / 跨资源依赖 | 暂无独立 docs 条目 | [06-requeue-mechanism.md](../../../skills/01-architecture/01-controller/06-requeue-mechanism.md)、[skills/07-review/SKILL.md](../../../skills/07-review/SKILL.md) | 这是典型的 agent-first 知识，应优先沉淀在 `skills/` |

## 命令归属规则

- 仓库级入口命令写在 [AGENTS.md](../../../AGENTS.md)。
- 工作流专属命令写在对应 `skills/*` 文档里。
- `docs/` 解释这些命令“为什么这样用”，但不要再维护一套重复的命令大全。

例如：

- `cargo build`、`cargo test`、集成测试入口命令在 `AGENTS.md`
- 集成测试目录结构、调试方式在 [skills/04-testing/01-integration-testing.md](../../../skills/04-testing/01-integration-testing.md)
- 面向团队成员的“怎么和 AI 配合使用这套能力”在 [ai-agent-collaboration.md](./ai-agent-collaboration.md)

## 维护规则

1. 更新共享事实时，先改权威位置，再由其他文档链接过去。
2. 不要把完整的大段实现说明同时复制进 `docs/` 和 `skills/`。
3. 新增高频 workflow 时，优先新增或改进 skill。
4. 新增长篇背景说明时，优先写进 `docs/`，然后在 skill 中链接。
5. 平台专属文件只能做入口和补充，不能长成第二套知识树。
6. 修改 `AGENTS.md`、`skills/` 或这层 dev-guide 入口文档后，执行 `make check-agent-docs`，及时发现坏链接和过期架构路径。

## 当前迁移优先级

建议下一阶段继续收敛这些内容：

1. 如果后续资源类型继续增多，只在出现重复模式时再补新的 reference 案例，不要继续膨胀主 workflow。
2. 视使用频率决定是否为 `work-directory` 抽独立 skill；不高频则继续保留在 `docs/`。
3. 如果 `logging` 的细节继续增长，优先把“规则清单”和“示例”沉到 skill/reference，而不是堆在总导航页。
