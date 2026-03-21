# Task 模板

> 创建新任务时，复制本模板到 `tasks/working/<task-name>/<task-name>.md`，填充具体内容。

---

```markdown
# <任务标题>

## 元信息

| 属性 | 值 |
|------|-----|
| 创建日期 | YYYY-MM-DD |
| 状态 | pending / in-progress / completed / blocked |
| 类型 | feature / bugfix / refactor / docs / config |
| 优先级 | P0 / P1 / P2 / P3 |
| 关联 Issue | #xxx 或 无 |
| 关联 Roadmap | 条目名称 或 无 |

## 原始需求

（原文引用或简要描述用户/产品的需求）

## 范围定义

### 本次范围内
- ...

### 明确排除
- ...

## 生命周期阶段

| 阶段 | Step 文件 | 状态 | 备注 |
|------|----------|------|------|
| 0 Intake | 本文件 | completed | — |
| 1 Analysis | step-01-analysis.md | pending | — |
| 2 Design | step-02-design.md | pending | — |
| 3 Implementation | step-03-implementation.md | pending | — |
| 4 Testing | step-04-testing.md | pending | — |
| 5 Documentation | step-05-documentation.md | pending | — |
| 6 Review | step-06-review.md | pending | — |

（如某阶段被裁剪，状态标为 `skipped`，在备注中说明原因）

## 受影响模块

| 模块 | 影响程度 |
|------|---------|

## 关键决策日志

| 日期 | 决策 | 原因 |
|------|------|------|

## 阻塞项

（如有阻塞，记录在此）
```

---

## 使用说明

### 创建任务

```bash
# AI 或人工执行
mkdir -p tasks/working/<task-name>
# 复制本模板内容到 tasks/working/<task-name>/<task-name>.md
# 填充元信息和原始需求
```

### 推进任务

每完成一个阶段：

1. 创建对应的 step 文件（参考 [SKILL.md](SKILL.md) 中各阶段的 step 规范）
2. 更新主任务文件中的阶段状态
3. 如有关键决策，追加到决策日志

### 关闭任务

1. 完成 step-06-review.md
2. 执行知识回写（参考 [task/SKILL.md](../09-task/SKILL.md) 的 Post-Task Checklist）
3. 将主任务文件状态改为 `completed`
4. 将任务目录从 `tasks/working/` 移到 `tasks/done/`（可选）

### Step 文件命名规范

```
step-01-analysis.md
step-02-design.md
step-03-implementation.md
step-04-testing.md
step-05-documentation.md
step-06-review.md
```

如某阶段需要拆分多个子步骤，使用后缀：

```
step-03-implementation-types.md
step-03-implementation-handler.md
step-03-implementation-api.md
```

### 与 lifecycle 的关系

本模板定义了 task 的**结构**，[SKILL.md](SKILL.md) 定义了每个阶段的**执行规范**。

AI agent 在执行任务时：

1. 首先读取 `skills/00-lifecycle/SKILL.md` 确定当前应处于哪个阶段
2. 按该阶段的规范创建 step 文件
3. 按该阶段的 skills 映射表加载所需的知识
4. 完成 step 文件中的门禁检查项
5. 更新主任务文件中的阶段状态
6. 进入下一阶段
