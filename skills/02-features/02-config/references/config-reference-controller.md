# Controller TOML 参考

> 适用于 `config/edgion-controller.toml` 以及 `EdgionControllerConfig`。

## 看哪些文件

- 样例配置：`config/edgion-controller.toml`
- 配置结构：`src/core/common/config/mod.rs`
- `conf_center` 顶层：`src/core/controller/conf_mgr/conf_center/config.rs`
- FileSystem 模式：`src/core/controller/conf_mgr/conf_center/file_system/config.rs`
- Kubernetes 模式：`src/core/controller/conf_mgr/conf_center/kubernetes/config.rs`

## 顶层 section 速查

| Section | 作用 | 关键字段 |
|---------|------|---------|
| `work_dir` | 运行目录 | 相对日志路径、`config/crd` 加载位置等 |
| `[server]` | 对外监听 | `grpc_listen`, `admin_listen` |
| `[logging]` | controller system log | `log_dir`, `log_level`, `json_format`, `console` |
| `[debug]` | 调试开关 | `enabled` |
| `[validation]` | 语义校验策略 | `enable_reference_grant_validation` |
| `[conf_sync]` | 资源同步与缓存 | `default_capacity`, `capacity_overrides`, `no_sync_kinds` |
| `[conf_center]` | 配置源模式 | `type = "file_system"` 或 `type = "kubernetes"` |

## `[server]`

| 字段 | 默认/常见值 | 说明 |
|------|-------------|------|
| `grpc_listen` | 代码 fallback `0.0.0.0:50051`，仓库样例也用这个 | Gateway 连接的 gRPC sync 地址 |
| `admin_listen` | 代码 fallback `0.0.0.0:8080`，仓库样例设成 `0.0.0.0:5800` | Controller Admin API |

注意：
- 实际日常开发里默认会加载 `config/edgion-controller.toml`，所以你通常看到的是 `5800`，不是代码 fallback 的 `8080`

## `[logging]`

| 字段 | 说明 |
|------|------|
| `log_dir` | system log 目录，通常写 `logs` |
| `log_prefix` | 文件名前缀，默认 `edgion-controller` |
| `log_level` | `trace/debug/info/warn/error` |
| `json_format` | 是否输出 JSON |
| `console` | 是否同时输出到控制台 |
| `buffer_size` | tracing appender buffer |

`log_dir` 会通过 `work_dir().resolve()` 解析，所以相对路径通常会落到 `work_dir/logs/...`。

## `[validation]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `enable_reference_grant_validation` | `true` | 控制跨命名空间引用是否按 ReferenceGrant 校验 |

## `[conf_sync]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `default_capacity` | `200` | 所有 kind 的 EventStore 默认容量 |
| `capacity_overrides` | 无 | 按资源种类覆盖容量，key 形如 `HTTPRoute` |
| `no_sync_kinds` | 默认走 `DEFAULT_NO_SYNC_KINDS` | 一旦配置，会整体替换默认列表 |

示例：

```toml
[conf_sync]
default_capacity = 200
no_sync_kinds = ["ReferenceGrant", "Secret"]

[conf_sync.capacity_overrides]
HTTPRoute = 1000
Gateway = 100
```

## `[conf_center]` 选项

### FileSystem 模式

```toml
[conf_center]
type = "file_system"
conf_dir = "examples/test/conf"
endpoint_mode = "auto"
```

字段：

| 字段 | 说明 |
|------|------|
| `type` | 固定 `file_system` |
| `conf_dir` | YAML 资源目录 |
| `endpoint_mode` | `endpoint` / `endpoint_slice` / `auto` |

重要现实约束：
- `conf_dir` 当前由 `FileSystemStorage::new()` 直接按进程 cwd 解析
- 也就是说，它并不像很多日志路径那样自动跟 `work_dir` 走
- 想避免歧义时，优先写绝对路径

CLI 特例：
- `--conf-dir` 会直接覆盖这里的 `conf_dir`
- 而且会把 `conf_center` 切到 FileSystem 变体，只保留原来的 `endpoint_mode`

### Kubernetes 模式

```toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"
controller_name = "edgion.io/gateway-controller"
watch_namespaces = []
label_selector = "app=edgion"
endpoint_mode = "auto"
gateway_address = "203.0.113.10"
ha_mode = "leader-only"
```

常用字段：

| 字段 | 说明 |
|------|------|
| `gateway_class` | 必填，Controller 负责的 GatewayClass |
| `controller_name` | Gateway API status 里回报的 controllerName |
| `watch_namespaces` | 空数组表示 watch all |
| `label_selector` | 仅处理满足标签条件的对象 |
| `endpoint_mode` | `endpoint` / `endpoint_slice` / `both` / `auto` |
| `gateway_address` | 当 Gateway status.addresses 没法自动决议时的静态 fallback |
| `ha_mode` | `leader-only` 或 `all-serve` |

### `[conf_center.leader_election]`

| 字段 | 默认值 |
|------|--------|
| `lease_name` | `edgion-controller-leader` |
| `lease_namespace` | `POD_NAMESPACE` 或 `default` |
| `lease_duration_secs` | `15` |
| `renew_period_secs` | `10` |
| `retry_period_secs` | `2` |

### `[conf_center.metadata_filter]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `blocked_annotations` | kubectl/helm 常见大注解 | 进入内存前移除 |
| `remove_managed_fields` | `true` | 减少对象体积 |

## 当前项目里最常见的 Controller 配置改动

- 本地测试切换配置目录：改 `conf_center.type = "file_system"` + `conf_dir`
- K8s 部署只管某个 GatewayClass：改 `gateway_class`
- 控制不同 kind 的缓存上限：改 `conf_sync.capacity_overrides`
- 调整观测噪音：改 `logging.log_level`

## 相关

- [../04-config-reference.md](../04-config-reference.md)
- [../../06-tracing/00-debugging.md](../../06-tracing/00-debugging.md)
