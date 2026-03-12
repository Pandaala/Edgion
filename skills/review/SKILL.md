---
name: review
description: Review-specific knowledge for Edgion. Use when auditing code review findings, especially to check whether a reported issue is a real bug, a project-accepted tradeoff, or a known false positive.
---

# Review 知识沉淀

本目录存放 Edgion 项目在 review 中可复用的结论，优先记录：

- 已结合代码确认的误报或过度定级
- 需要按项目设计语义判断的问题
- 下次 review 时可直接复用的判定标准

按需读取：

- 判定内存问题时，先看 [memory-leak/not-a-memory-leak.md](memory-leak/not-a-memory-leak.md)

使用原则：

- 只有当结论已经结合当前代码和设计语义核实过，才写入这里
- 内容保持简短，只保留可复用判断，不复制整份 review
- 如果未来设计目标变化，要同步更新这里的结论
