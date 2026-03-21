# Step 01 — 需求分析

## 需求理解

将两个相关但分散的 skill 目录合并为一个：
- `skills/03-coding-standards/`（2 个文件：日志 ID 传播、日志安全）
- `skills/05-observability/`（3 个文件：access log 规范、metrics 规范、tracing 规范）

合并后改名为 `skills/03-coding/`，成为统一的编码规范目录。

## 影响分析

### 当前文件清单

**03-coding-standards/ (将成为 03-coding/)**
```
SKILL.md                         — 编码规范导航
00-logging-and-tracing-ids.md    — 日志 ID 传播（rv/sv/key_name）
01-log-safety.md                 — 日志安全（不泄密、不泄配置、数据面不打 tracing）
```

**05-observability/ (将被合并)**
```
SKILL.md                         — 可观测性导航（信息分层、自检清单）
00-access-log.md                 — Access Log 字段设计、PluginLog 格式
01-metrics.md                    — Metrics 规范（无 Histogram、Label 约束）
02-tracing-and-logging.md        — 控制面 tracing 结构化规范
```

### 引用这两个目录的完整文件清单

以下文件中包含指向 `03-coding-standards/` 或 `05-observability/` 的链接，合并后全部需要更新：

**skills/ 内部（相对路径 `../03-coding-standards/` 或 `../05-observability/`）：**
1. `skills/SKILL.md` — 总导航，有两个完整分类段落
2. `skills/00-lifecycle/SKILL.md` — Phase 3 加载列表、编码规范检查点、阶段映射表
3. `skills/00-lifecycle/02-skills-directory-design.md` — 编号体系、依赖图
4. `skills/02-features/SKILL.md` — 引用 `03-coding-standards/SKILL.md` 和 `05-observability/00-access-log.md`
5. `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` — 引用 `05-observability/00-access-log.md`
6. `skills/01-architecture/02-gateway/11-conf-handler-guidelines.md` — 引用 `03-coding-standards/01-log-safety.md`
7. `skills/05-observability/00-access-log.md` — 引用 `03-coding-standards/00-logging-and-tracing-ids.md`（合并后变为同目录内引用）
8. `skills/05-observability/02-tracing-and-logging.md` — 引用 `03-coding-standards/` 两个文件（合并后变为同目录内引用）
9. `skills/09-task/SKILL.md` — step 表格引用 `coding-standards/`
10. `skills/01-architecture/05-plugin-system.md` — 引用 `05-observability/00-access-log.md`

**项目根目录：**
11. `AGENTS.md` — Knowledge Map 有两个条目，Common Workflows 有一个引用

**docs/ 目录：**
12. `docs/en/dev-guide/knowledge-source-map.md` — 引用 `05-observability/`
13. `docs/zh-CN/dev-guide/knowledge-source-map.md` — 引用 `05-observability/`

**注意：** `tools/validate_agent_docs.py` 中**没有**引用这两个目录（已验证）。

## 约束与假设

- 假设编号 `05` 空出后不回填，不影响其他目录编号
- 假设 observability 的 3 个文件内部无需修改内容，只需修改文件间的交叉引用
- 合并后的 SKILL.md 需要融合两份 SKILL.md 的内容（导航 + 信息分层 + 自检清单）

## 风险

- 无重大风险。纯文档移动操作，不涉及代码变更

## 待确认项

- 无（需求明确）
