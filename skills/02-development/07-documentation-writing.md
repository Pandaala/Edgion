# 文档编写指南

用于这些任务：

- 为新功能编写面向用户的文档
- 向 `docs/` 添加或更新页面
- 确保双语文档（en + zh-CN）格式和风格一致
- 通过文档审查检查清单

## 文档层级说明

Edgion 有两层文档：

| 层 | 位置 | 面向对象 |
|----|------|---------|
| `skills/` | 项目内 AI/开发者知识库 | AI Agent 和开发者 |
| `docs/` | 面向用户的文档 | 最终用户 |

**不要在两层之间重复详细内容**，选择一个规范来源后在另一层链接。

## 双语文档规范

当前 `docs/` 支持双语（en + zh-CN）：

- 英文文档：`docs/en/`
- 中文文档：`docs/zh-CN/`
- 新增页面需要同时提供两个语言版本
- i18n 规则和术语表见 [../11-doc/03-i18n-rules.md](../11-doc/03-i18n-rules.md)

## 文档编写工作流

1. **确认文档类型**：概念介绍、操作指南还是参考文档
2. **阅读页面模板**：[../11-doc/02-page-template.md](../11-doc/02-page-template.md)
3. **遵循写作风格**：[../11-doc/01-writing-style.md](../11-doc/01-writing-style.md)
4. **提交前自查**：[../11-doc/05-review-checklist.md](../11-doc/05-review-checklist.md)
5. **版本说明规范**：涉及版本差异时参考 [../11-doc/04-versioning-and-changelog.md](../11-doc/04-versioning-and-changelog.md)

## 常见踩坑

- 忘记同步更新英文和中文两个版本
- `skills/` 里的技术细节直接复制到 `docs/` 中，造成维护两份
- 代码示例使用了已废弃的注解或 API，需要和开发代码保持同步

## 参考文档

- 文档规范导航：[../11-doc/SKILL.md](../11-doc/SKILL.md)
- 文档结构概览：[../11-doc/00-doc-structure.md](../11-doc/00-doc-structure.md)
- 写作风格：[../11-doc/01-writing-style.md](../11-doc/01-writing-style.md)
- 页面模板：[../11-doc/02-page-template.md](../11-doc/02-page-template.md)
- i18n 规则：[../11-doc/03-i18n-rules.md](../11-doc/03-i18n-rules.md)
- 版本和 changelog：[../11-doc/04-versioning-and-changelog.md](../11-doc/04-versioning-and-changelog.md)
- 审查清单：[../11-doc/05-review-checklist.md](../11-doc/05-review-checklist.md)
