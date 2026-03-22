---
name: 07-tasks
description: 在仓库 tasks/ 目录下创建、更新或继续任务时使用此 skill。涵盖任务结构、文档集、生命周期阶段、状态跟踪和任务模板。
---

# 任务管理

当工作需要记录到 `tasks/` 时使用此 skill。

## 目录规则

- `tasks/working/` — 进行中的工作
- `tasks/todo/` — 待办 / 想法
- `tasks/done/` — 已完成归档
- 每个活跃任务有独立目录，使用 kebab-case 命名（如 `log-tracing-optimization`）

## 任务文档集

每个任务目录包含以下文档，按需创建（仅主任务文件为必需）：

```text
tasks/working/<task-name>/
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

### 各文档职责

#### 主任务文件 `<task-name>.md`（必需）

任务入口，包含元信息和 AI 引导摘要。AI 首次接手任务时先读此文件，快速了解全貌。

#### `00-context.md` — 关联 Skills 索引（推荐）

列出本任务涉及的所有 skills 文档路径，让 AI 编码时能快速定位参考资料，无需从头检索。

```markdown
# 关联 Skills

## 架构参考
- `skills/01-architecture/02-gateway/05-plugin-system.md` — 插件执行流程
- `skills/01-architecture/05-resources/13-edgion-plugins.md` — 插件资源处理链路

## 功能参考
- `skills/02-features/03-resources/13-edgion-plugins.md` — 插件配置 Schema

## 编码规范
- `skills/03-coding/00-logging-and-tracing-ids.md` — 日志 ID 规范

## 测试参考
- `skills/05-testing/00-integration-testing.md` — 集成测试框架
```

#### `01-design.md` — 总设计文档（推荐）

架构方案、接口设计、数据流、配置变更方案。大型任务的技术蓝图。

#### `02-implementation.md` — 实现与测试（推荐）

代码实现方案和测试策略合并在一起，因为二者紧密关联：

```markdown
# 实现与测试

## 实现方案
- 修改文件清单及各文件的改动摘要
- 关键代码逻辑说明

## 测试策略
- 单元测试：覆盖哪些模块、边界条件
- 集成测试：覆盖哪些场景
- 手动验证：需要哪些手动检查

## 测试覆盖率评估

| 模块 | 已有覆盖 | 新增覆盖 | 风险区域 |
|------|---------|---------|---------|
```

#### `03-subtasks.md` — 子任务拆分与进度（按需）

当任务较大时，拆分为可独立完成的子任务，每个子任务带状态跟踪：

```markdown
# 子任务

## 总进度

已完成 2/5

## 子任务清单

### ST-01: 定义新的 CRD 类型
- **状态**: completed
- **分支**: feature/new-crd-types
- **产出**: `src/types/resources/new_resource.rs`

### ST-02: 实现 Handler
- **状态**: in-progress
- **当前阻塞**: 等待 ST-01 的类型定义合并
```

#### `04-todo.md` — 待办事项（按需）

零散的、不构成独立子任务的待处理项。与 `03-subtasks.md` 的区别：子任务是有明确产出的工作单元，todo 是细碎的待办。

#### `05-issues.md` — 关键问题（按需）

当前无法立即解决的阻塞项、critical issue、需要他人协助的问题：

```markdown
# 关键问题

## 未解决

### ISSUE-01: Gateway 热重载时插件状态丢失
- **严重度**: critical
- **发现日期**: 2026-03-20
- **影响**: 热重载后插件的内存缓存被清空
- **临时方案**: 暂无
- **需要**: 与 Gateway 核心团队讨论状态持久化方案

## 已解决

### ISSUE-02: ...
- **解决方案**: ...
- **解决日期**: 2026-03-21
```

#### `06-decisions.md` — 决策记录（按需）

重要的技术决策和权衡取舍。当决策较多、主任务文件的决策表放不下时使用。

#### `07-changelog.md` — 变更日志（按需）

记录关键里程碑，适用于长周期任务。短任务无需此文件。

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

## 生命周期阶段 — 各步骤职责

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

任务完成后，将产出同步到对应位置：

| 产出 | 目标位置 | 时机 |
|--------|--------|------|
| 可复用模式 | `skills/` | 具有通用性的方案 |
| 审查结论 | `skills/04-review/` | 项目特定的可复用发现 |
| 用户文档 | `docs/en/`、`docs/zh-CN/` | 用户可见的行为变更 |
| 架构决策 | `skills/01-architecture/` | 系统设计变更 |
| 新资源/插件文档 | `docs/*/dev-guide/` | 新增资源或插件 |

## 默认工作流

1. 检查任务是否已存在于 `tasks/working/` 或 `tasks/todo/`
2. 创建任务目录和主任务文件
3. 创建 `00-context.md`，列出相关 skills 路径
4. 按生命周期阶段推进，按需创建对应文档
5. 发现阻塞问题立即写入 `05-issues.md`
6. 随工作推进更新主任务文件中的文档索引和状态
7. 任务完成后执行完成检查清单，归档到 `tasks/done/`
