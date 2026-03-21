# Skills 目录体系设计

> 本文档记录 `skills/` 目录体系的设计原则、编号规范和维护规则。修改目录结构时必须参照本文。

## 设计目标

1. **可预测的加载顺序** — 编号前缀确保 AI agent 按依赖顺序发现和加载 skills
2. **生命周期对齐** — 编号顺序与开发生命周期阶段对应，从需求到交付
3. **渐进式披露** — 三级结构：根 SKILL.md → 分类 SKILL.md → 具体文件
4. **单一职责** — 每个目录只覆盖一个知识领域，避免交叉

## 编号体系

```
skills/
├── 00-lifecycle        # 生命周期本身（元知识，最先加载）
├── 01-architecture     # 系统架构（基础层，被其他所有 skill 依赖）
├── 02-development      # 开发指南（构建在架构之上）
├── 03-coding # 编码规范（编码阶段的约束）
├── 04-testing          # 测试验证（编码之后）
├── 06-cicd             # 构建发布（交付阶段）
├── 07-review           # 代码审查（质量保障，贯穿全程但独立沉淀）
├── 08-gateway-api      # 专项知识（Gateway API 兼容性）
├── 09-task             # 任务管理（执行载体）
├── 10-misc             # 杂项（不属于以上类别）
└── SKILL.md            # 总导航文件
```

### 编号规则

| 规则 | 说明 |
|------|------|
| 两位数字前缀 | `00-` 到 `99-`，保留扩展空间 |
| `00` 保留给元知识 | 生命周期、目录设计等"关于 skills 本身"的知识 |
| `01-06` 对应开发流程 | 架构 → 开发 → 编码规范（含可观测性） → 测试 → CI/CD |
| `07-08` 横切关注点 | 代码审查、专项兼容性 |
| `09` 任务管理 | 执行载体 |
| `10+` 杂项与扩展 | 不影响已有编号 |

### 依赖方向

```
00-lifecycle ──引用──▶ 所有其他 skills（阶段映射表）
01-architecture ──被引用──▶ 02-development, 04-testing, 08-gateway-api
02-development ──引用──▶ 01-architecture, 03-coding, 04-testing, 06-cicd
03-coding ──被引用──▶ 02-development, 07-review（含 observability/ 子目录）
04-testing ──引用──▶ 02-development, 10-misc
06-cicd ──引用──▶ 02-development
07-review ──引用──▶ 03-coding（可观测性审查规则）
08-gateway-api ──独立──
09-task ──引用──▶ 00-lifecycle
10-misc ──独立──
```

**原则：低编号不依赖高编号**（`01-architecture` 不应引用 `04-testing`），例外是 `00-lifecycle` 作为元知识引用所有。

## 目录内部结构规范

每个 skill 目录的标准结构：

```
XX-skill-name/
├── SKILL.md              # 必需。目录导航 + frontmatter（name, description）
├── 00-topic.md           # 编号内容文件
├── 01-topic.md
├── ...
└── references/           # 可选。参考材料子目录（无 SKILL.md）
```

### SKILL.md frontmatter 规范

```yaml
---
name: skill-name          # kebab-case，与目录名（去掉编号前缀）一致
description: >            # 一句话说明：什么时候加载这个 skill
  Use this skill when ...
---
```

### 内容文件编号规则

- 使用两位数字前缀：`00-`, `01-`, `02-`, ...
- `00` 通常是概述（overview）
- 编号反映阅读顺序或重要性，不必连续
- `references/` 子目录内的文件不编号

## 新增 Skill 目录流程

1. 确定知识领域是否与现有目录重叠，优先扩展现有目录
2. 选择编号：
   - 如果是开发流程的一部分，插入到 `01-06` 的合适位置
   - 如果是横切关注点，使用 `07-08` 范围
   - 如果是扩展知识，使用 `10+`
3. 创建目录和 SKILL.md
4. 更新 `skills/SKILL.md` 总导航
5. 更新 `AGENTS.md` Knowledge Map
6. 更新 `00-lifecycle/SKILL.md` 的阶段映射表（如果新 skill 属于某个阶段）
7. 更新本文档的编号体系和依赖图

## 重命名 / 重组规则

- 重命名目录时**必须**同时更新所有引用（使用全局搜索 `skills/旧名称/`）
- 受影响的引用位置：
  - `skills/SKILL.md` — 总导航
  - `AGENTS.md` — Knowledge Map 和 Common Workflows
  - `skills/00-lifecycle/SKILL.md` — 阶段映射表
  - `skills/` 内所有 `../旧名称/` 相对引用
  - `docs/` 内所有 `skills/旧名称/` 绝对引用
  - `tools/validate_agent_docs.py` — ENTRY_FILES 列表
  - `.cursor/rules/` — 如有引用
- 使用 `git mv` 重命名以保留 git 历史

## 与 docs/ 的关系

```
skills/  →  面向 AI agent 的任务知识（流程、规范、架构原理）
docs/    →  面向人类用户的文档（使用指南、运维手册、API 参考）
```

| 属性 | skills/ | docs/ |
|------|---------|-------|
| 受众 | AI agent + 开发者 | 最终用户 + 运维 |
| 风格 | 精炼、结构化、可机器解析 | 叙事性、示例丰富 |
| 语言 | 中英混用 | 中英分目录 |
| 更新时机 | 实现完成后立即更新 | 功能稳定后更新 |
| 重复内容 | 禁止与 docs/ 重复，用链接指向 | 禁止与 skills/ 重复，用链接指向 |
