---
name: lifecycle
description: Use this skill as the first entry point when starting any task. It defines the full software development lifecycle that every task must follow, the gate criteria between phases, and how AI agents should navigate the skills knowledge base at each phase.
---

# Lifecycle Skill — AI 驱动的软件开发生命周期

> 本文件是所有任务的**起点**。每个 task 在 `tasks/working/` 下创建时，必须按照本生命周期流程执行。

## 核心理念

传统开发靠人的经验串联各阶段。本体系将生命周期**编码为可执行的 skill 链**，让 AI 在每个阶段自动加载正确的上下文，确保：

1. **不遗漏** — 每个阶段有明确的输入、输出和门禁检查
2. **可追溯** — 从需求到代码到测试到文档，全链路可回溯
3. **可复现** — 任何 AI agent 读取 task 文件即可继续工作

## 生命周期阶段总览

```
┌─────────────────────────────────────────────────────────┐
│  Phase 0: Intake — 任务受理                              │
│  输入: 用户需求 / Issue / Roadmap 条目                    │
│  输出: tasks/working/<name>/<name>.md (主任务文件)        │
├─────────────────────────────────────────────────────────┤
│  Phase 1: Analysis — 需求分析                            │
│  输入: 主任务文件                                         │
│  输出: step-01-analysis.md                               │
│  加载: skills/01-architecture/, 相关 docs/                   │
├─────────────────────────────────────────────────────────┤
│  Phase 2: Design — 方案设计                              │
│  输入: step-01 分析结论                                   │
│  输出: step-02-design.md                                 │
│  加载: skills/01-architecture/, skills/02-development/          │
├─────────────────────────────────────────────────────────┤
│  Phase 3: Implementation — 编码实现                       │
│  输入: step-02 设计方案                                   │
│  输出: step-03-implementation.md + 代码变更                │
│  加载: skills/02-development/, skills/03-coding/      │
├─────────────────────────────────────────────────────────┤
│  Phase 4: Testing — 测试验证                              │
│  输入: 代码变更                                           │
│  输出: step-04-testing.md + 测试代码                      │
│  加载: skills/04-testing/                                    │
├─────────────────────────────────────────────────────────┤
│  Phase 5: Documentation — 文档更新                        │
│  输入: 已验证的功能                                       │
│  输出: step-05-documentation.md + docs/ 变更              │
│  加载: skills/02-development/07-documentation-writing.md     │
├─────────────────────────────────────────────────────────┤
│  Phase 6: Review & Close — 回顾与关闭                     │
│  输入: 全部 step 完成                                     │
│  输出: step-06-review.md + skills/ 知识回写               │
│  加载: skills/07-review/                                     │
└─────────────────────────────────────────────────────────┘
```

## Phase 0: Intake — 任务受理

**触发条件：** 用户提出需求、Bug 报告、Roadmap 条目需要执行。

**AI 操作：**

1. 读取本文件 (`skills/00-lifecycle/SKILL.md`)
2. 检查 `tasks/working/` 是否已有相关任务
3. 创建任务目录和主任务文件，使用 [task 模板](01-task-template.md)
4. 在主任务文件中记录原始需求、初步范围、生命周期阶段状态

**门禁：** 主任务文件创建完毕，用户确认范围。

## Phase 1: Analysis — 需求分析

**目的：** 理解"做什么"和"为什么做"，定位受影响的模块和边界。

**AI 操作：**

1. 加载 `skills/01-architecture/SKILL.md`，理解系统架构
2. 按需深入相关架构文件（配置中心、数据面、插件系统等）
3. 分析需求对现有模块的影响范围
4. 创建 `step-01-analysis.md`

**step-01 必须包含：**

```markdown
## 需求理解
（用自己的话复述需求，确保理解正确）

## 影响分析
| 模块 | 影响程度 | 说明 |
|------|---------|------|

## 现有相关实现
（已有代码 / 已有类似功能的参考）

## 约束与假设
## 风险
## 待确认项
```

**门禁：** 用户确认需求理解正确、影响范围无遗漏。

## Phase 2: Design — 方案设计

**目的：** 确定"怎么做"——接口、数据结构、流程、配置项。

**AI 操作：**

1. 加载 `skills/01-architecture/` 中相关子系统文件
2. 加载 `skills/02-development/SKILL.md` 获取开发规范
3. 如涉及新资源，加载 `skills/02-development/00-add-new-resource.md`
4. 如涉及插件，加载 `skills/02-development/01-edgion-plugin-dev.md` 或 `02-stream-plugin-dev.md`
5. 创建 `step-02-design.md`

**step-02 必须包含：**

```markdown
## 方案概述
（一段话说清楚整体方案）

## 接口设计
（新增 / 修改的 trait、struct、API）

## 数据流
（从输入到输出的完整路径，建议用文本流程图）

## 配置变更
（新增的配置项、YAML/TOML 字段）

## 变更文件清单
| 文件 | 变更类型 | 说明 |
|------|---------|------|

## 备选方案
（考虑过但放弃的方案及原因）

## 风险
## 待确认项
```

**门禁：** 用户确认设计方案，变更范围可控。

## Phase 3: Implementation — 编码实现

**目的：** 将设计转化为代码。

**AI 操作：**

1. 加载 `skills/03-coding/SKILL.md`
2. 加载 `skills/02-development/` 中相关的开发规范
3. 按 step-02 的变更文件清单逐一实现
4. 创建 `step-03-implementation.md` 记录实现决策

**step-03 必须包含：**

```markdown
## 实现概要
（实际编码中做出的关键决策）

## 代码变更记录
| 文件 | 变更摘要 |
|------|---------|

## 与设计的偏差
（如有，说明原因）

## 已知技术债务
## 待完善项
```

**编码规范检查点：**
- [ ] 符合 `skills/03-coding/00-logging-and-tracing-ids.md`
- [ ] 符合 `skills/03-coding/01-log-safety.md`
- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --all-targets` 无 warning

**门禁：** 代码编译通过，lint 检查通过。

## Phase 4: Testing — 测试验证

**目的：** 验证实现的正确性和健壮性。

**AI 操作：**

1. 加载 `skills/04-testing/SKILL.md`
2. 编写单元测试（与业务代码同文件的 `#[cfg(test)]` 模块）
3. 编写集成测试（如需要，按 `skills/04-testing/00-integration-testing.md`）
4. 运行测试并记录结果
5. 创建 `step-04-testing.md`

**step-04 必须包含：**

```markdown
## 测试策略
（说明为什么选择这些测试方法）

## 单元测试
| 测试 | 文件 | 覆盖场景 |
|------|------|---------|

## 集成测试
| 测试套件 | 配置目录 | 覆盖场景 |
|---------|---------|---------|

## 测试结果
## 覆盖率变化
（如有 coverage 工具，记录变更前后的覆盖率）

## 未覆盖的场景
（明确哪些没测、为什么没测）
```

**门禁：** `cargo test --all` 通过，新增代码有对应测试。

## Phase 5: Documentation — 文档更新

**目的：** 更新用户文档和开发者文档。

**AI 操作：**

1. 加载 `skills/02-development/07-documentation-writing.md`
2. 判断需要更新的文档类型：
   - 用户文档 → `docs/en/` + `docs/zh-CN/`
   - 开发者文档 → `docs/*/dev-guide/`
   - 运维文档 → `docs/*/ops-guide/`
3. 创建 `step-05-documentation.md`

**step-05 必须包含：**

```markdown
## 文档变更清单
| 文件 | 变更类型 | 说明 |
|------|---------|------|

## 新增文档
（如有新增，说明放置位置和理由）

## 待更新的关联文档
（本次未更新但应当后续更新的文档）
```

**门禁：** 用户可见的功能变更有对应文档。

## Phase 6: Review & Close — 回顾与关闭

**目的：** 回顾整个任务，将可复用的知识回写到 skills/。

**AI 操作：**

1. 加载 `skills/07-review/SKILL.md`
2. 执行 task skill 的 [Post-Task Checklist](../09-task/SKILL.md)
3. 创建 `step-06-review.md`

**step-06 必须包含：**

```markdown
## 完成摘要
（一段话总结本任务完成了什么）

## 知识回写
| 输出 | 目标位置 | 状态 |
|------|---------|------|

## 模块状态更新
（本任务涉及的模块是否需要更新 TODO / 成熟度 / 已知问题）

## 经验教训
（本任务中学到的、值得记录的经验）
```

**门禁：** 知识回写完成，主任务文件标记为 completed。

## 快速参考：阶段 → Skills 映射

| 阶段 | 必读 Skills | 按需 Skills |
|------|------------|------------|
| 0 Intake | `00-lifecycle/SKILL.md`, `09-task/SKILL.md` | — |
| 1 Analysis | `01-architecture/SKILL.md` | 具体架构子文件 |
| 2 Design | `01-architecture/`, `02-development/SKILL.md` | `08-gateway-api/`, `02-development/references/` |
| 3 Implementation | `03-coding/SKILL.md`, `02-development/` | `03-coding/observability/` |
| 4 Testing | `04-testing/SKILL.md` | `04-testing/references/` |
| 5 Documentation | `02-development/07-documentation-writing.md` | `docs/DIRECTORY.md` |
| 6 Review | `07-review/SKILL.md`, `09-task/SKILL.md` | — |

## 阶段可裁剪性

并非所有任务都需要完整走完 6 个阶段。裁剪规则：

| 任务类型 | 必须阶段 | 可跳过 |
|---------|---------|--------|
| 新功能 | 全部 | — |
| Bug 修复 | 0, 1, 3, 4 | 2(如修复明显), 5(如无用户影响), 6 |
| 重构 | 0, 1, 2, 3, 4 | 5(如无外部变化), 6 |
| 文档补全 | 0, 5 | 1-4, 6 |
| 配置变更 | 0, 1, 3, 4 | 2, 6 |

裁剪时在主任务文件中标注跳过的阶段及原因。
