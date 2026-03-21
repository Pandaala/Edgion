# Step 02 — 方案设计

## 方案概述

将 `05-observability/` 的 3 个内容文件移入 `03-coding-standards/`，对合并后的文件统一重编号，融合两份 SKILL.md，然后将目录从 `03-coding-standards` 重命名为 `03-coding`，最后更新所有 13 个引用文件。

## 合并后的目录结构

```
skills/03-coding/
├── SKILL.md                         ← 融合两份 SKILL.md
├── 00-logging-and-tracing-ids.md    ← 原 03-coding-standards/00（不变）
├── 01-log-safety.md                 ← 原 03-coding-standards/01（不变）
├── 02-access-log.md                 ← 原 05-observability/00-access-log.md
├── 03-metrics.md                    ← 原 05-observability/01-metrics.md
└── 04-tracing-and-logging.md        ← 原 05-observability/02-tracing-and-logging.md
```

编号逻辑：
- `00-01`：编码强制规范（原 coding-standards，偏"禁止什么"）
- `02-04`：可观测性规范（原 observability，偏"怎么记录"）

## 执行步骤

### Step A: 移动文件（使用 git mv 保留历史）

```bash
cd /Users/caohao/ws1/Edgion

# 移动 observability 的 3 个内容文件到 coding-standards，同时重编号
git mv skills/05-observability/00-access-log.md skills/03-coding-standards/02-access-log.md
git mv skills/05-observability/01-metrics.md skills/03-coding-standards/03-metrics.md
git mv skills/05-observability/02-tracing-and-logging.md skills/03-coding-standards/04-tracing-and-logging.md

# 删除 observability 的 SKILL.md（内容将融合到 coding-standards 的 SKILL.md）
git rm skills/05-observability/SKILL.md

# 删除空目录（git rm 后目录应自动消失，如果没有则手动 rmdir）
rmdir skills/05-observability 2>/dev/null || true

# 重命名目录
git mv skills/03-coding-standards skills/03-coding
```

### Step B: 融合 SKILL.md

将 `03-coding/SKILL.md` 重写，融合：
- 原 `coding-standards/SKILL.md` 的 frontmatter 和编码规范描述
- 原 `observability/SKILL.md` 的信息分层表格和提交前自检清单
- 更新 frontmatter 的 name 为 `coding`，description 扩展涵盖 observability

### Step C: 修复合并文件内部的交叉引用

合并后 `02-access-log.md` 和 `04-tracing-and-logging.md` 中原来引用 `../03-coding-standards/` 的链接变为同目录内引用：

| 文件 | 旧引用 | 新引用 |
|------|--------|--------|
| `02-access-log.md` | `../03-coding-standards/00-logging-and-tracing-ids.md` | `00-logging-and-tracing-ids.md` |
| `04-tracing-and-logging.md` | `../03-coding-standards/00-logging-and-tracing-ids.md` | `00-logging-and-tracing-ids.md` |
| `04-tracing-and-logging.md` | `../03-coding-standards/01-log-safety.md` | `01-log-safety.md` |

### Step D: 更新外部引用（13 个文件）

所有替换规则：

| 旧路径 | 新路径 |
|--------|--------|
| `03-coding-standards/SKILL.md` | `03-coding/SKILL.md` |
| `03-coding-standards/00-logging-and-tracing-ids.md` | `03-coding/00-logging-and-tracing-ids.md` |
| `03-coding-standards/01-log-safety.md` | `03-coding/01-log-safety.md` |
| `05-observability/SKILL.md` | `03-coding/SKILL.md` |
| `05-observability/00-access-log.md` | `03-coding/02-access-log.md` |
| `05-observability/01-metrics.md` | `03-coding/03-metrics.md` |
| `05-observability/02-tracing-and-logging.md` | `03-coding/04-tracing-and-logging.md` |

注意相对路径前缀：
- skills/ 内部文件使用 `../03-coding/` 或 `03-coding/`（根 SKILL.md）
- AGENTS.md 使用 `skills/03-coding/`
- docs/ 文件使用 `../../../skills/03-coding/`

**需更新的文件完整清单：**

1. `skills/SKILL.md` — 删除 observability 段落，重写 coding-standards 段落为 coding
2. `skills/00-lifecycle/SKILL.md` — 更新 Phase 3 加载路径、检查点路径、阶段映射表
3. `skills/00-lifecycle/02-skills-directory-design.md` — 更新编号体系、依赖图
4. `skills/02-development/SKILL.md` — 更新两处引用
5. `skills/02-development/01-edgion-plugin-dev.md` — 更新 observability 引用
6. `skills/02-development/08-conf-handler-guidelines.md` — 更新 coding-standards 引用
7. `skills/09-task/SKILL.md` — 更新 step 表格引用
8. `skills/01-architecture/05-plugin-system.md` — 更新 observability 引用
9. `AGENTS.md` — 合并两个 Knowledge Map 条目为一个，更新 Workflows 引用
10. `docs/en/dev-guide/knowledge-source-map.md` — 更新 observability 引用
11. `docs/zh-CN/dev-guide/knowledge-source-map.md` — 更新 observability 引用

### Step E: 验证

```bash
# 验证无残留旧引用
grep -rn '05-observability\|03-coding-standards' --include='*.md' --include='*.py' --include='*.mdc' | grep -v '\.git/'

# 预期结果：空（零匹配）
```

## 备选方案

| 方案 | 放弃原因 |
|------|---------|
| 保留 observability 为独立目录，只加交叉引用 | 用户明确要求合并，且内容确实属于编码规范 |
| 合并但保留 `coding-standards` 名称 | 合并后范围超出了 "standards"，`coding` 更简洁 |
| 回填编号让 05 不空 | 连锁重命名代价大，空编号无害 |

## 风险

- 无重大风险

## 待确认项

- 无
