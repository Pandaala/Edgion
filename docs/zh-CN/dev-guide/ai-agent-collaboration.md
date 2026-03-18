# AI 协作与 Skills 使用指南

本文档说明在 Edgion 仓库中如何让 AI 正确使用 `AGENTS.md`、`skills/`、`docs/` 和命令，而不需要每次手动把一堆文档贴给它。

## 核心原则

- `AGENTS.md` 是跨平台统一入口。
- `skills/` 是任务型知识库，不是整套开发文档的镜像。
- `docs/` 是给人看的文档，保留长篇解释、多语言版本和面向贡献者/用户的材料。
- AI 协作时优先让 Agent 自己沿着导航找资料，而不是你手动指定十几个文件。

## 新需求时怎么让 AI 用 Skills

推荐直接这样说：

```text
请先按 AGENTS.md 和 skills/SKILL.md 自主定位相关 skill，再开始分析和实现。
这个需求需要先理解架构。
```

或者更具体一点：

```text
请先看 AGENTS.md，然后从 skills/SKILL.md 进入 architecture 和 development，
只加载和这个需求直接相关的文档，再给方案并实现。
```

如果是测试或排障：

```text
请按 AGENTS.md 使用 testing skill 和相关的 Gateway API / TLS 调试资料。
```

这类提示比“我把 8 篇文档都发给你”更稳，因为它把“找资料”也交给了 Agent。

## 是不是应该只有一个大的 Skill

不建议做成一个巨大的 `SKILL.md`。

更好的结构是三层：

1. 仓库总入口：`AGENTS.md`
2. 总导航：`skills/SKILL.md`
3. 领域导航和任务 skill：`skills/<domain>/SKILL.md` 和具体参考文件

原因很简单：

- 大单文件很快失控，AI 每次都要吞掉很多无关上下文。
- 小 skill 更容易触发准确，也更容易维护。
- 领域导航能减少“每个具体 skill 都重复写背景”的问题。

## 每次聊天都要带上整体导航吗

如果工具会读取仓库内指令文件，通常不需要你每次手动贴整体导航。

更推荐的做法是：

- 在仓库根维护好 `AGENTS.md`
- 在 `AGENTS.md` 中明确要求先从 `skills/SKILL.md` 进入
- 只有在某个平台不自动读取仓库指令时，你才额外在聊天里说一句“请先按 AGENTS.md 工作”

也就是说，重点不是“有没有命令让它读导航”，而是“仓库里有没有稳定的导航入口”。

## 有没有统一命令

没有一个跨所有平台都通用的 slash command。

真正稳定的跨平台机制是：

- 根目录 `AGENTS.md`
- 结构化的 `skills/`
- 必要时的薄封装文件，如 `CLAUDE.md`、`.cursor/rules/00-edgion-entry.mdc`

你可以把常用提示词固定成短句：

- `请先按 AGENTS.md 工作`
- `请从 skills/SKILL.md 自主定位相关技能`
- `这个任务需要先理解架构，再改代码`

这已经足够接近“统一命令”了，而且跨平台可用。

## 命令应该放哪里

建议分层放：

- `AGENTS.md`：只放最常用的仓库级命令
- 具体 `skill`：放工作流专属命令
- `docs/`：给人看的命令使用说明

例如：

- `cargo build`、`cargo test`、集成测试入口放在 `AGENTS.md`
- 某个测试套件的细节放在 `skills/testing/`
- 给团队成员看的“怎么和 AI 配合”放在本文档

不要把所有命令都堆进一个超级 skill，也不要在 `docs/` 和 `skills/` 里重复维护同一套命令解释。

## 知识库和命令怎么兼容

兼容的关键是“命令是 workflow 的一部分，不是单独一层知识库”。

推荐约定：

- 任务型知识以 skill 为主
- 命令跟随 skill 或 `AGENTS.md`
- 长篇解释留在 docs
- 同一条命令只保留一个权威位置

例如集成测试：

- 仓库级入口命令写在 `AGENTS.md`
- 套件细节、目录结构、排障方法写在 `skills/testing/00-integration-testing.md`
- 面向人的“如何和 AI 一起使用这些测试能力”写在本文档

## 当前仓库的推荐使用方式

- 先让 AI 读 `AGENTS.md`
- 再让它从 `skills/SKILL.md` 找对应领域
- 需要具体实现时再读取对应 skill 的细节文件
- 只有当你知道某篇文档特别关键时，再显式点名

## 维护建议

- 新增高频工作流时，优先新增或改进 skill，而不是先写一篇孤立文档。
- 新增长篇背景说明时，优先写到 `docs/`，再在相关 skill 中链接它。
- 如果某个平台需要专属规则文件，让它们做薄封装，不要自己长成第二套知识库。
