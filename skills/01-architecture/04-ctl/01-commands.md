---
name: ctl-commands
description: edgion-ctl 子命令详解：apply、delete、get、reload 的使用方式和实现细节。
---

# edgion-ctl 子命令

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### apply

<!-- TODO:
- 从文件或目录应用配置
- 仅支持 center target
- 支持 --dry-run
- 实现：读取 YAML → 调用 ConfCenter API 创建/更新
-->

### delete

<!-- TODO:
- 按 kind/name 或从文件删除资源
- 仅支持 center target
-->

### get

<!-- TODO:
- 查询资源，支持命名空间和名称过滤
- 支持三种 target 模式
- 输出格式：table（默认）、json、yaml、wide
-->

### reload

<!-- TODO:
- 从存储重新加载所有资源
- 仅支持 center target
- 触发 Controller 全量重处理
-->
