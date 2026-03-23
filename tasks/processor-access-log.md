# Controller Processor Access Log 设计方案

> 状态：待实施
> 日期：2026-03-22

## 背景

数据面已有完整的 `AccessLogEntry` 结构化 JSON 日志，控制面缺少对等物。当前 `process_resource()` 的 11 个步骤中散布着 `tracing::debug/warn/trace`，一个资源的处理过程无法用一条记录看完。

## 目标

在 `process_resource()` 完整处理流程中收集一条 **`ProcessorLog`**，处理结束时一次性输出结构化 JSON。

## ProcessorLog 结构

```rust
#[derive(Serialize)]
pub struct ProcessorLog {
    pub ts: i64,                          // 时间戳
    pub rsc: Rsc,                         // 资源标识 (kind/ns/name)，复用已有 Rsc 结构
    pub rv: u64,                          // resourceVersion
    pub phase: &'static str,              // "init" | "runtime"
    pub result: &'static str,             // "processed" | "skipped" | "deleted"

    // 各步骤结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ns_filter: Option<bool>,          // namespace filter (false=被过滤)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler_filter: Option<bool>,     // handler filter

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub validate_errors: Vec<String>,     // validate 阶段错误
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub preparse_errors: Vec<String>,     // preparse 阶段错误

    pub parse_result: &'static str,       // "continue" | "skip"

    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_changed: Option<bool>,     // status 语义变更检测结果

    #[serde(skip_serializing_if = "Option::is_none")]
    pub requeue_triggered: Option<Vec<String>>,  // on_change 触发的 requeue 目标

    // 耗时（微秒）
    pub total_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_change_us: Option<u64>,
}
```

## 插入点

在 `processor.rs` 的 `process_resource()` 开头创建，各步骤填充，末尾输出：

```
process_resource() {
    let mut plog = ProcessorLog::new(kind, ns, name, rv, phase);

    // 1. namespace filter → plog.ns_filter = Some(false); return
    // 2. handler filter   → plog.handler_filter = Some(false); return
    // 3. clean metadata   (无需记录)
    // 4. validate         → plog.validate_errors = errors.clone()
    // 5. preparse         → plog.preparse_errors = errors.clone()
    // 6. parse            → plog.parse_result; plog.parse_us
    // 7-9. status         → plog.status_changed
    // 10. on_change       → plog.on_change_us
    // 11. save            (无需记录)

    plog.finish();  // total_us, 序列化输出
}
```

`process_delete()` 同样输出一条，`result = "deleted"`，字段更少。

## 输出方式

建议用 `tracing::info!(target: "processor_log", ...)` + tracing subscriber filter 分离到独立文件。

优点：无需新增基础设施，可通过配置路由到文件/ES/stdout。

## 不动的地方

- 现有 `tracing::warn/debug` 保留（开发调试用，与 processor_log 互补）
- Handler trait 不改（日志收集全在 processor.rs 层）
- 不在 metadata 上附加日志字段（processor_log 是侧输出，不进入资源本身）

## 待确认

1. `requeue_triggered` 是否记录（需改 `HandlerContext::requeue` 收集记录）
2. 是否需要记录 `clean_metadata` 移除了哪些字段
3. init 阶段是否需要特殊处理（批量资源时日志量大）

## 涉及文件

| 文件 | 改动 |
|------|------|
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` | 主要改动：嵌入 ProcessorLog 收集逻辑 |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/mod.rs` | 新增 ProcessorLog 结构定义 |
| `src/types/ctx.rs` | 复用 Rsc 结构（已完成） |
