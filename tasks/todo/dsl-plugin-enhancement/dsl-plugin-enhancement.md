# DSL 插件增强

> 补强 EdgionDSL 自定义脚本插件，使其成为可用的差异化竞争功能。
> Priority: P2

## Background

DSL 插件架构完备（Parser→AST→Compiler→Bytecode→VM，共 4688 行），沙箱安全模型完善。
当前主要缺口：4 个 stub 函数未实现（sha256/md5/regex_find/regex_replace），缺少 resp 读取、break/continue、ResponseFilter 阶段支持。
做好了可以成为类似 APISIX Lua / Envoy Wasm 的差异化功能，让用户无需写 Rust 插件就能定制逻辑。

## Status

- [x] 研究阶段完成
- [x] 设计文档完成
- [ ] 实现

## 文档

| 文档 | 状态 |
|------|------|
| `step-01-research.md` | ✅ 完成（深度源码分析，4688 行）|
| 设计文档 | ✅ `docs/superpowers/specs/2026-03-22-dsl-plugin-enhancement-design.md` |

## Steps

| Step | Content | Status |
|------|---------|--------|
| step-01 | 现状分析与能力评估 | completed |
| step-02 | 方案设计 | completed |
| step-03 | 实现（分3个子步骤） | pending |
| step-04 | 测试 | pending |
