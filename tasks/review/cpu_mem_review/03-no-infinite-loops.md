# 死循环审查结论

> 本次审查未发现死循环风险。以下记录已有的安全机制。

---

## 已有保护机制

### 1. TriggerChain 环检测

**文件**: `src/core/controller/conf_mgr/sync_runtime/workqueue.rs:186-199`

- `max_trigger_depth = 20`: 触发链最大深度
- `max_trigger_cycles = 5`: 同一 (kind, key) 最多重复触发次数
- 到达上限时终止触发链并记录警告日志

### 2. Workqueue 有界 + 退避

- 队列容量: 5000
- 重试有指数退避
- max_retries 配置上限

### 3. ACME 服务重试限制

- `MAX_RETRY_ATTEMPTS = 5`
- 指数退避
- 资源删除后正确清理 retry_tracker

### 4. DSL VM Sandbox Budget

- step budget: 限制总执行步数
- call budget: 限制函数调用次数
- loop budget: 限制循环迭代次数 (100)
- stack depth limit: 限制递归深度
- string length limit: 限制字符串长度
- `catch_unwind` 隔离: 防止 panic 扩散

### 5. Plugin Ref 循环检测

**文件**: `src/types/ctx.rs:668-687`

- `plugin_ref_stack` + `has_plugin_ref()` 检测循环引用
- `DEFAULT_PLUGIN_REF_DEPTH = 5` 硬限制

### 6. Consistent Hash Walking

- `max_iterations` 限制遍历次数
- 超出后回退到默认选择

### 7. IP Radix Tree 查找

- IPv4: bit 位数限制 (32)
- IPv6: bit 位数限制 (128)
- child_index == 0 终止

### 8. Watcher Error 退避

**文件**: `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs`

- 错误后指数退避，上限 60 秒

### 9. Etcd Health Monitor 退避

- 500ms 起步，上限 30s
- 成功后重置

### 10. gRPC 重连退避

- Gateway ConfigSyncClient 在 Watch 断开后有退避重连机制
- 不会 busy-loop

---

## 建议改进（非必须）

1. **TriggerChain 参数可调低**: 当前 depth=20 + cycles=5 允许的最大级联工作量较大。可考虑收紧至 depth=10, cycles=3。
2. **`plugin_ref_stack` 添加显式深度拒绝**: 当前仅做循环检测，无独立深度上限。建议 `push_plugin_ref()` 达到 10-20 层时返回错误。
3. **`ProcessorRegistry::wait_kinds_ready()`**: 100ms 轮询可改为 Notify-based，仅启动时影响，优先级低。
