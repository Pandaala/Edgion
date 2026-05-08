---
name: 07-tasks
description: 在仓库 tasks/ 目录下创建、更新或继续任务时使用此 skill。涵盖任务结构、文档集、生命周期阶段、状态跟踪和任务模板。
---

# 任务管理

当工作需要记录到 `tasks/` 时使用此 skill。

## 快速入口

| 场景 | 直接入口 |
|------|----------|
| 新功能完整交付工作流（分析 → 设计 → 设计复审 → 实现 → 代码复审 → 测试补强 → 全量验证 → 总结） | [new_feature_work_flow.skill](new_feature_work_flow.skill) |
| 新功能工作流输出模板 | [new_feature_work_flow_templates.md](new_feature_work_flow_templates.md) |
| 新功能工作流 subagent 契约 | [new_feature_work_flow_subagents.md](new_feature_work_flow_subagents.md) |

## 目录规则

- `tasks/pending/` — 进行中或待启动的工作
- `tasks/review/` — 进入复审 / 验收阶段
- `tasks/todo/` — 待办 / 想法（尚未启动）
- `tasks/todo-test-cover/` — 测试覆盖治理专项（独立工作流，按编号顺序推进）
- `tasks/done/` — 已完成归档
- 任务粒度灵活：单文件 `<task-name>.md` 或独立目录均可，命名使用 kebab-case（如 `log-tracing-optimization`）；复杂任务（多文档集）使用目录形式。

## 任务文档集

每个任务目录包含以下文档，按需创建（仅主任务文件为必需）：

```text
tasks/pending/<task-name>/
├── <task-name>.md              # [必需] 主任务文件：元信息、需求、范围、AI 引导摘要
├── 00-context.md               # [推荐] 关联 Skills 索引：本任务涉及的 skills 路径列表
├── 01-design.md                # [推荐] 总设计文档：架构方案、接口定义、数据流
├── 02-implementation.md        # [推荐] 实现与测试：代码方案、测试策略、覆盖率评估
├── 03-subtasks.md              # [按需] 子任务拆分与进度跟踪
├── 04-todo.md                  # [按需] 待办事项：零散的待处理项
├── 05-issues.md                # [按需] 关键问题：阻塞项、无法立即解决的 critical issue
├── 06-decisions.md             # [按需] 决策记录：重要决策及其原因
└── 07-changelog.md             # [按需] 变更日志：关键里程碑和变更记录
```

| 文件 | 作用 | 何时创建 | 备注 |
|------|------|----------|------|
| `<task-name>.md` | 任务入口，元信息 + AI 引导摘要 | 任务开始时，必需 | AI 首次接手先读此文件 |
| `00-context.md` | 列出本任务涉及的 skills 路径，方便 AI 编码时快速定位 | 进入实现阶段前 | 推荐，确保 AI 有路径可循 |
| `01-design.md` | 架构方案、接口设计、数据流、配置变更 | 设计阶段 | 大型任务的技术蓝图 |
| `02-implementation.md` | 代码方案 + 测试策略 + 覆盖率评估（合并，因二者紧密关联） | 实现阶段 | 推荐 |
| `03-subtasks.md` | 子任务拆分 + 每个子任务状态跟踪 | 改动超过 3 模块或 5 文件时 | 含状态、分支、产出字段 |
| `04-todo.md` | 零散待处理项，不构成独立子任务 | 按需 | 与 `03-subtasks.md` 区别：无明确产出 |
| `05-issues.md` | 当前无法立即解决的阻塞项 / critical issue | 发现阻塞时立即创建 | 按需；含严重度、临时方案、需要字段 |
| `06-decisions.md` | 重要技术决策与权衡取舍 | 决策较多时从主文件溢出 | 按需 |
| `07-changelog.md` | 关键里程碑记录 | 长周期任务 | 短任务无需 |

## 主任务文件模板

```markdown
# <任务标题>

## 元信息

| 键 | 值 |
|-----|-------|
| 创建日期 | YYYY-MM-DD |
| 状态 | pending / in-progress / completed / blocked |
| 类型 | feature / bugfix / refactor / docs / config |
| 优先级 | P0 / P1 / P2 / P3 |
| Issue | #xxx 或 N/A |

## AI 引导摘要

> 一段话说明：这个任务要做什么、为什么做、核心难点是什么。
> AI 首次接手时读这段即可理解任务全貌。

## 需求

（原始需求，原文或摘要）

## 范围

### 包含
- ...

### 不包含
- ...

## 文档索引

| 文档 | 状态 | 说明 |
|------|------|------|
| 00-context.md | ✅ | 关联 Skills 索引 |
| 01-design.md | ✅ | 总设计 |
| 02-implementation.md | 🔄 | 实现与测试 |
| 03-subtasks.md | 🔄 | 子任务进度 |
| 04-todo.md | — | 未创建 |
| 05-issues.md | ⚠️ | 有 1 个未解决 |
| 06-decisions.md | — | 未创建 |
| 07-changelog.md | — | 未创建 |

## 影响模块

| 模块 | 影响 |
|--------|--------|

## 决策记录

| 日期 | 决策 | 原因 |
|------|----------|--------|
```

## 生命周期阶段

| 阶段 | 目的 | 加载 Skills | 产出文档 |
|-------|---------|-------------|----------|
| 1 分析 | 理解需求，识别影响模块 | `01-architecture/SKILL.md` | `00-context.md` |
| 2 设计 | 定义接口、数据流、配置变更 | `01-architecture/`、`02-features/SKILL.md` | `01-design.md` |
| 3 实现 | 按设计编码 | `01-architecture/`（开发指南）、`02-features/`（配置 Schema）、`03-coding/SKILL.md` | `02-implementation.md`、`03-subtasks.md` |
| 4 审查 | 代码审查 | `04-review/SKILL.md` | `06-decisions.md` |
| 5 测试 | 验证正确性 | `05-testing/SKILL.md` | 更新 `02-implementation.md` 测试覆盖率 |

### 阶段裁剪

| 任务类型 | 必需阶段 | 可跳过 |
|-----------|----------------|----------|
| 新功能 | 全部 | — |
| Bug 修复 | 1、3、5 | 2（若方案明显）、4 |
| 重构 | 1、2、3、4 | 5（若无外部变更） |
| 仅文档 | 3 | 1、2、4、5 |
| 配置变更 | 1、3、5 | 2、4 |

## 文档创建规则

1. **按需创建**：只创建当前阶段需要的文档，不要预创建空文件
2. **主文件先行**：任何任务至少有主任务文件 `<task-name>.md`
3. **context 优先**：进入实现阶段前，`00-context.md` 应该就绪，确保 AI 有路径可循
4. **子任务阈值**：预估改动超过 3 个模块或 5 个文件时，创建 `03-subtasks.md` 拆分
5. **issue 即时记录**：发现阻塞问题立即写入 `05-issues.md`，不要等到后面才补

## 状态值

- `pending` — 未开始
- `in-progress` — 进行中
- `completed` — 已完成
- `blocked` — 等待依赖
- `skipped` — 跳过（需注明原因）

## 实现步骤检查点

- [ ] 遵循 `skills/03-coding/00-logging-and-tracing-ids.md`
- [ ] 遵循 `skills/03-coding/01-log-safety.md`
- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --all-targets` 无警告

## 任务完成检查清单

| 产出 | 目标位置 | 时机 |
|--------|--------|------|
| 可复用模式 | `skills/` | 具有通用性的方案 |
| 审查结论 | `skills/04-review/` | 项目特定的可复用发现 |
| 用户文档 | `docs/en/`、`docs/zh-CN/` | 用户可见的行为变更 |
| 架构决策 | `skills/01-architecture/` | 系统设计变更 |
| 新资源/插件文档 | `docs/*/dev-guide/` | 新增资源或插件 |

## 默认工作流

1. 检查任务是否已存在于 `tasks/pending/`、`tasks/todo/` 或 `tasks/review/`
2. 创建任务目录或单文件，并写入主任务文件
3. 创建 `00-context.md`，列出相关 skills 路径
4. 新功能任务优先使用 [new_feature_work_flow.skill](new_feature_work_flow.skill) 作为执行模板
5. 按生命周期阶段推进，按需创建对应文档
6. 发现阻塞问题立即写入 `05-issues.md`
7. 随工作推进更新主任务文件中的文档索引和状态
8. 进入验收阶段移到 `tasks/review/`；通过后归档到 `tasks/done/`
