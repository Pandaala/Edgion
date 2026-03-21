# 合并 observability 到 coding-standards，并改名为 coding

## 元信息

| 属性 | 值 |
|------|-----|
| 创建日期 | 2026-03-21 |
| 状态 | pending |
| 类型 | docs |
| 优先级 | P1 |
| 关联 Issue | 无 |
| 关联 Roadmap | skills 目录体系重构 |

## 原始需求

将 `skills/05-observability/` 的内容整合到 `skills/03-coding-standards/` 中，合并后目录改名为 `skills/03-coding/`。原因：observability 的内容（access log 规范、metrics 规范、tracing 规范）本质上属于编码规范的一部分，拆成两个独立 skill 增加了 AI 加载的复杂度。

## 范围定义

### 本次范围内

1. 将 `skills/05-observability/` 下的 3 个文件合并到 `skills/03-coding-standards/`
2. 合并后的目录从 `03-coding-standards` 重命名为 `03-coding`
3. 合并 SKILL.md：将 observability 的 SKILL.md 内容（信息分层、提交前自检清单）整合到 coding 的 SKILL.md 中
4. 删除空的 `skills/05-observability/` 目录
5. 重编号内容文件：合并后的文件按统一编号排列
6. 更新所有引用这两个目录的文件
7. 更新 `skills/00-lifecycle/02-skills-directory-design.md` 中的编号体系和依赖图

### 明确排除

- 不修改文件的实质内容（只做移动、重命名、引用更新）
- 不重构其他 skill 目录
- 不调整 `04-testing`、`06-cicd` 等目录的编号（`05` 空出来即可，或后续单独处理）

## 生命周期阶段

| 阶段 | Step 文件 | 状态 | 备注 |
|------|----------|------|------|
| 0 Intake | 本文件 | completed | — |
| 1 Analysis | step-01-analysis.md | completed | 见下方 |
| 2 Design | step-02-design.md | completed | 见下方 |
| 3 Implementation | — | pending | 执行重构操作 |
| 4 Testing | — | skipped | 纯文档任务，无代码测试 |
| 5 Documentation | — | skipped | 本任务本身就是文档重构 |
| 6 Review | — | pending | 验证所有引用无残留 |

## 受影响模块

| 模块 | 影响程度 |
|------|---------|
| `skills/03-coding-standards/` | 高 — 接收文件，改名为 `03-coding/` |
| `skills/05-observability/` | 高 — 整体合并后删除 |
| `skills/SKILL.md` | 高 — 总导航需要重写这两个分类 |
| `skills/00-lifecycle/SKILL.md` | 中 — 阶段映射表中引用了这两个目录 |
| `skills/00-lifecycle/02-skills-directory-design.md` | 中 — 编号体系和依赖图需更新 |
| `AGENTS.md` | 中 — Knowledge Map 引用了这两个目录 |
| `skills/02-development/SKILL.md` | 低 — 引用了 `05-observability/` |
| `skills/02-development/01-edgion-plugin-dev.md` | 低 — 引用了 `05-observability/00-access-log.md` |
| `skills/02-development/08-conf-handler-guidelines.md` | 低 — 引用了 `03-coding-standards/01-log-safety.md` |
| `skills/01-architecture/05-plugin-system.md` | 低 — 引用了 `05-observability/00-access-log.md` |
| `skills/09-task/SKILL.md` | 低 — step 表格引用了 `coding-standards/` |
| `docs/en/dev-guide/knowledge-source-map.md` | 低 — 引用了 `05-observability/` |
| `docs/zh-CN/dev-guide/knowledge-source-map.md` | 低 — 引用了 `05-observability/` |

## 关键决策日志

| 日期 | 决策 | 原因 |
|------|------|------|
| 2026-03-21 | 合并而非保留双目录 | observability 内容本质是编码规范，拆分增加加载复杂度 |
| 2026-03-21 | 改名为 `coding` 而非保留 `coding-standards` | 合并后范围更广，`coding` 更简洁准确 |
| 2026-03-21 | `05` 编号空出不回填 | 避免连锁重命名，空编号无害 |

## 阻塞项

无
