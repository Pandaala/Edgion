# New Feature Work Flow Subagents

用于 `new_feature_work_flow.skill` 的 subagent 协作约束。目标是减少越权实现、重复设计和低质量 review。

## 通用规则

- 主 agent 负责最终收敛，不能无条件接受 subagent 结论
- 多个 subagent 不能同时修改同一组文件
- subagent 若发现设计不成立，应回传阻塞，不应自行改方向
- subagent 输出必须是结论化、可执行的，不要泛泛建议

## 设计 Review Subagent

输入：

- 原始需求摘要
- 当前设计摘要
- 相关 skill 路径
- 指定重点风险域

目标：

- 找设计漏洞、遗漏、违背现有架构处

禁止事项：

- 不要重写整份设计
- 不要直接开始实现
- 不要给无结论的泛化建议

输出模板：

```markdown
## Design Review Findings

### Must Fix
- [严重度] 问题：
  - 原因：
  - 建议修正：

### Nice to Have
- 问题：
  - 建议：

### Assumptions To Verify
- ...
```

## 实现 Subagent

输入：

- 已定稿的设计
- 明确文件所有权
- 明确禁止改动范围

目标：

- 在指定文件范围内完成实现与直接相关单元测试

禁止事项：

- 不要自行改设计目标
- 不要顺手重构无关文件
- 不要修改未授权文件

输出模板：

```markdown
## Implementation Result
- 修改文件：
- 核心实现：
- 补充测试：
- 遇到的问题：
```

## 代码 Review Subagent

输入：

- 最终实现 diff
- 需求与设计摘要
- 重点关注模块

目标：

- 识别实现后残留问题、行为偏差、隐藏风险

禁止事项：

- 不要重复步骤 3 的设计讨论
- 不要给纯风格建议，除非影响正确性、维护性或安全性

输出模板：

```markdown
## Code Review Findings

### Must Fix
- [文件/逻辑点] 问题：
  - 影响：
  - 修复建议：

### Should Fix
- ...

### Acceptable Risk
- ...
```

## 主 Agent 收敛规则

- `Must Fix` 默认必须修
- `Should Fix` 若不修，需记录原因
- `Acceptable Risk` 必须进入最终总结
- 若 subagent 结论与代码事实不符，以主 agent 复核结果为准
