# Edgion Documentation Writing Guide

用于这些任务：

- 新增或重写 `docs/` 下的人类文档
- 调整 `AGENTS.md`、`skills/`、`docs/` 的分工边界
- 补“功能已改，但文档、索引、示例、AI 入口没跟上”的收尾工作

先读这些权威入口：

- [../../docs/zh-CN/dev-guide/knowledge-source-map.md](../../docs/zh-CN/dev-guide/knowledge-source-map.md)
- [../../docs/zh-CN/dev-guide/ai-agent-collaboration.md](../../docs/zh-CN/dev-guide/ai-agent-collaboration.md)
- [../../docs/zh-CN/dev-guide/README.md](../../docs/zh-CN/dev-guide/README.md)
- [../../docs/en/dev-guide/README.md](../../docs/en/dev-guide/README.md)
- [../../docs/DIRECTORY.md](../../docs/DIRECTORY.md)

## 先决定“写到哪里”

不要一上来就在 `docs/` 里加新文件。先判断这份知识属于哪一层：

- `AGENTS.md`：跨平台共享入口、仓库级规则、最常用命令
- `skills/`：Agent 会反复执行的 workflow、排障路径、项目特有约束
- `docs/`：写给人的长篇解释、背景、设计稿、多语言文档
- 平台专属薄封装：`CLAUDE.md`、`.cursor/rules/00-edgion-entry.mdc`，只做入口，不长成第二套知识库

经验规则：

- 如果内容回答的是“AI 该怎么做这件事”，优先放 `skills/`
- 如果内容回答的是“人为什么这样设计 / 如何理解整体”，优先放 `docs/`
- 如果内容只是高频命令入口，优先放 `AGENTS.md`

## 再决定“写给谁”

当前仓库的人类文档主要分成四类：

| 文档区 | 读者 | 应写什么 |
|--------|------|----------|
| `docs/*/user-guide/` | 使用路由、插件和配置能力的用户 | 怎么配置、效果是什么、限制和示例 |
| `docs/*/ops-guide/` | 运维、平台、SRE | Gateway / TLS / 监听器 / 观测 / 部署运维 |
| `docs/*/dev-guide/` | 贡献者、维护者 | 架构、实现边界、扩展 workflow、源码心智模型 |
| `docs/review/` / 设计稿 | 评审者和维护者 | 设计过程、评审结论、阶段性方案，不把它伪装成通用手册 |

不要把“用户怎么配”和“维护者怎么实现”写在同一篇文档里。

## 当前语言规则

当前稳定维护的贡献者文档入口是：

- `docs/zh-CN/dev-guide/`
- `docs/en/dev-guide/`

`docs/ja/` 目前不是并行维护的完整文档树，不要默认需要同步。

写文档时按这个规则处理：

- 改动 `dev-guide` 的核心入口文档时，优先保持 `zh-CN` 和 `en` 同步
- `user-guide` / `ops-guide` 优先跟随同目录现有语言覆盖，不强行新开不成体系的语言副本
- 如果暂时只能补一种语言，在文档或提交说明里明确它是“先补主版本，后续再补对照语言”，不要假装已经双语齐备

## 功能改动时，文档要跟哪些事实对齐

文档不能独立漂着。写或改文档时，优先对齐这些事实源：

- Rust 类型与实现：`src/`
- CRD / manifest：`config/crd/`
- 示例与测试 YAML：`examples/`
- 仓库级 workflow：`AGENTS.md`
- Agent workflow：`skills/`

尤其是新增资源、插件、注解、字段时，至少确认下面几件事：

1. 代码里已经有对应实现，或者文档明确标记为“设计稿 / 未落地”
2. 如果用户会写 YAML，这个字段在 CRD 或 schema 中确实存在
3. 如果文档给了示例，示例路径、资源名、注解键、命令要和当前仓库一致
4. 如果改的是高频 workflow，要同步检查对应 `skills/` 和 `AGENTS.md`

不要写出“代码和 schema 里还没有，但文档已经像正式特性一样宣称可用”的内容。

## 推荐写作 workflow

### 1. 先找权威位置

先问自己：

- 这条事实应该由 `docs/`、`skills/` 还是 `AGENTS.md` 维护？
- 仓库里已经有没有同主题的权威文档？
- 我是在补背景说明，还是在补 workflow？

有重叠主题时，优先更新权威位置，再让其他入口链接过去。

### 2. 选最近的现有文档做模板

优先复用同目录已有文档的结构、语气和层级，不要重新发明一套文风。

常见参考：

- 新的开发说明：看 `docs/*/dev-guide/`
- 新的用户配置说明：看 `docs/*/user-guide/`
- 新的 agent workflow：看 `skills/02-development/`、`skills/04-testing/`

### 3. 用“文档类型”决定结构，不要强行套一个超大模板

不同文档应该用不同结构：

- 架构概览：背景、模块边界、主链路、关键约束
- workflow：适用场景、入口文件、步骤、常见坑、验证方式
- 配置说明：最小示例、字段表、行为细节、限制、排障
- 设计评审稿：目标、方案、边界、待定项、审核结论

不要把所有文档都写成“概述 + 20 个固定章节”的格式。

### 4. 把隐含行为写出来

只要行为会影响用户配置、请求结果、重载、依赖解析或状态呈现，就应该写出来，例如：

- 默认值
- 顺序要求
- 注解键和作用范围
- controller 预处理或自动补全
- `null` / `[]` / 不设置 的差异
- 限制条件和失败表现

如果用户或维护者会因为某个行为感到意外，这个行为就不该藏在代码里。

### 5. 示例必须能对上当前仓库

写 YAML 或命令时，优先复用当前仓库里的真实命名：

- 资源 kind / group / version
- 示例目录
- 端口和路径
- 测试命令
- 注解 key

如果没有真实示例可对，就先去代码或 `examples/` 里确认，不要拍脑袋补示例。

### 6. 收尾时补索引

只要新增或删除了文档，至少检查：

- 对应目录 `README.md`
- [../../docs/DIRECTORY.md](../../docs/DIRECTORY.md)
- 如果是 AI 协作 / knowledge layer 入口，再检查 `knowledge-source-map.md`

不要新增正文后忘记更新入口索引。

## 命令与知识库的分工

命令不要到处复制。按这条规则处理：

- 仓库级常用命令放 `AGENTS.md`
- 工作流专属命令放对应 `skills/*`
- `docs/` 解释这些命令为什么这样用、适用于什么场景

如果你在文档里写了命令，应该确认它没有和 `AGENTS.md`、相关 skill 形成第二份冲突版本。

## 版本与变更说明怎么写

默认不要在每篇文档里到处加“since vX.Y.Z”。

只有这些情况才建议显式写版本或阶段信息：

- 功能受 feature flag / build feature 控制
- 某行为只在某次架构迁移后成立
- 文档本身是设计稿、阶段稿或 review 记录
- 用户升级时必须知道兼容性边界

更常见的做法是：

- 在“当前限制 / 当前状态”里写清楚
- 在设计稿里标注“未落地 / 待实现”
- 在 changelog 或 release 文档记录版本粒度的变化

## 质量检查清单

提交前至少过一遍：

- 目标读者是否明确
- 文档位置是否正确
- 示例和命令是否对得上当前仓库
- 是否把隐含逻辑写明了
- 是否和现有 skill / AGENTS / docs 分工冲突
- 新增或删除文件后，索引是否更新
- 相关入口改动后，是否运行了 `make check-agent-docs`

## 最小验证

修改 `AGENTS.md`、`skills/`、`docs/*/dev-guide/` 入口页或知识映射后，至少运行：

```bash
make check-agent-docs
```

如果改动涉及示例命令、脚本路径、CRD 文件名，最好再做一次定向搜索：

```bash
rg -n "your-feature|your-command|your-annotation" docs skills examples config/crd
```

## 什么时候需要同步更新 skill

出现这些情况时，不要只改 `docs/`：

- 新增了高频开发 workflow
- 排障路径变了
- 仓库级命令入口变了
- AI 需要知道的新约束并不容易从代码里推出来

这时通常至少还要看：

- [SKILL.md](SKILL.md)
- [../../skills/SKILL.md](../../skills/SKILL.md)
- [../../AGENTS.md](../../AGENTS.md)
