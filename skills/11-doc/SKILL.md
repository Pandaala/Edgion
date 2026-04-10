---
name: doc-standards
description: 用户文档写作规范。撰写或审阅 docs/ 下的文档时使用此 skill，涵盖目录结构、写作风格、页面模板、多语言、版本管理和 Review 检查清单。
---

# 用户文档规范 — Documentation Standards

> 适用于 `docs/` 目录下所有面向人类用户的文档。`skills/` 面向 AI 和开发者，不受本规范约束。

## 核心原则

1. **用户优先** — 文档面向使用者（运维、开发者、集成商），不是内部实现者
2. **示例驱动** — 每个功能页必须有完整可用的 YAML 示例，"看完能用"
3. **渐进披露** — 快速开始 → 详细配置 → 高级场景 → FAQ，按需深入
4. **多语言对称** — en/ 和 zh-CN/ 保持完全一致的目录和文件结构
5. **不重复 skills/** — docs/ 不讲内部实现，需要时链接到 dev-guide 或外部文档

## 文件清单

| # | 文件 | 主题 |
|---|------|------|
| 00 | [00-doc-structure.md](00-doc-structure.md) | 文档目录结构：四层体系、目录命名、README.md 规范、DIRECTORY.md 同步、新增文档放置决策 |
| 01 | [01-writing-style.md](01-writing-style.md) | 写作风格 + YAML 示例 + 交叉引用：标题规则、人称语气、Edgion 扩展标注、YAML 完整性与占位符、链接规范 |
| 02 | [02-page-template.md](02-page-template.md) | 页面模板 + 参数表 + FAQ 规范：插件页、Route 页、运维页、Getting Started 页的章节结构，参数表四列格式，FAQ Q/A 格式 |
| 03 | [03-i18n-rules.md](03-i18n-rules.md) | 多语言：en/zh-CN 对称规则、源语言约定、术语表、同步工作流、中文排版 |
| 04 | [04-versioning-and-changelog.md](04-versioning-and-changelog.md) | 文档版本管理：变更触发规则、Breaking Change 标注、弃用标注、与 Release Notes 关系 |
| 05 | [05-review-checklist.md](05-review-checklist.md) | 文档 Review 检查清单：技术准确性、完整性、多语言同步、链接有效性、安全审查 |

## 快速决策

| 你要做… | 入口 |
|---------|------|
| 写一个新插件的文档页 | [02-page-template.md](02-page-template.md) §插件页模板 |
| 决定新文档放在哪个目录 | [00-doc-structure.md](00-doc-structure.md) §放置决策树 |
| 写 YAML 示例 | [01-writing-style.md](01-writing-style.md) §YAML 示例规范 |
| 新增文档时处理双语 | [03-i18n-rules.md](03-i18n-rules.md) §同步工作流 |
| 审阅别人写的文档 | [05-review-checklist.md](05-review-checklist.md) |
| 写配置参数表 | [02-page-template.md](02-page-template.md) §配置参数表规范 |
