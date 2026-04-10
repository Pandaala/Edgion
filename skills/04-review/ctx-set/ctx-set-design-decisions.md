# ctx_set 插件 Review 判定规则

> 来源：2026-04 插件安全审计，逐一对照源码核实后的结论。

---

## ${ctx:name} 规则间交叉读取是设计意图，不是漏洞

**误判场景**：ctx_set 的 `run_request` 按顺序执行 vars 列表中的每条 rule，每条 rule 处理完后立即调用 `session.set_ctx_var()`。后续 rule 的模板中使用 `${ctx:name}` 可以读取前面 rule 设置的值，形成隐式数据流。

**实际情况**：

1. **这是 `${ctx:name}` 语法的核心价值**：如果不允许规则间读取，`${ctx:name}` 就没有存在意义——from/header/query 等其他源已足够覆盖单条规则的需求。
2. **行业标准做法**：APISIX 的 `set-by-lua`/变量系统、Kong 的 `ctx` 共享内存、Envoy 的 dynamic metadata 均允许插件/规则间读取已设置变量，顺序依赖是已知语义。
3. **无注入风险**：SEC-01 已修复（模板预编译），用户输入通过 `CompiledTemplate` 结构传递，运行时不解析 `${}`，不存在二次展开攻击路径。`test_template_injection_prevention` 测试覆盖了此场景。

**判定**：设计意图，不需要修复。文档应明确说明"rules 按声明顺序执行，后续 rule 可通过 `${ctx:name}` 读取前面 rule 设置的变量"。

---

## 模板注入（SEC-01）已通过预编译修复

**原问题**：`resolve_template` 用 `while let Some(start) = result.find("${")` 循环替换，替换后的值如果包含 `${...}` 会被二次展开。攻击者发送 `X-User: ${header:Authorization}` 即可泄露其他请求头。

**修复方式**：配置加载时预编译模板为 `Vec<TemplateSegment>`（`Literal` / `Variable`），运行时只做 segment 遍历拼接，完全不做 `find("${")` 搜索。等同于 SQL prepared statement 的思路——结构和数据彻底分离。

**涉及文件**：
- `src/types/resources/edgion_plugins/plugin_configs/ctx_set.rs` — `CompiledTemplate`、`TemplateSegment`
- `src/core/gateway/plugins/http/ctx_set/plugin.rs` — `evaluate_template`、`resolve_var`

**判定**：已修复，未来 review 中如果再看到 ctx_set 模板相关问题，先确认是否已使用预编译路径。
