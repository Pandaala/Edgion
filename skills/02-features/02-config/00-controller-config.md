---
name: controller-config
description: edgion-controller TOML 配置完整 Schema。
---

# Controller TOML 配置 Schema

> 文件路径默认：`config/edgion-controller.toml`，通过 `--config-file` 指定。

## 完整 Schema

```toml
# 工作目录（所有相对路径基于此目录）
# 优先级：CLI --work-dir > ENV EDGION_WORK_DIR > 此值 > "."
work_dir = "."

[server]
grpc_listen = "0.0.0.0:50051"    # gRPC 监听地址（ConfigSyncServer）
admin_listen = "0.0.0.0:5800"    # Admin HTTP API 地址

[logging]
log_dir = "logs"                  # 日志目录
log_prefix = "edgion-controller"  # 日志文件前缀
log_level = "info"                # trace | debug | info | warn | error
                                  # 支持模块级别：info,edgion::core::controller=debug
json_format = false               # JSON 结构化日志
console = true                    # 同时输出到控制台
buffer_size = 10000               # 日志缓冲区大小

[debug]
enabled = true                    # 启用调试功能

[validation]
enable_reference_grant_validation = true   # 启用 ReferenceGrant 跨命名空间校验

[conf_sync]
default_capacity = 200            # EventStore 默认容量（所有资源类型）
# 特定资源类型容量覆盖
# 注意：设置 no_sync_kinds 会完全替换默认列表，不是追加
[conf_sync.capacity_overrides]
# HTTPRoute = 500
# Service = 1000
# no_sync_kinds = ["ReferenceGrant", "Secret"]   # 不同步到 Gateway 的资源

# ─── FileSystem 模式 ───
[conf_center]
type = "filesystem"
conf_dir = "config/resources"              # YAML 资源目录
endpoint_mode = "EndpointSlice"            # EndpointSlice | Endpoints | Both

# ─── Kubernetes 模式 ───
# [conf_center]
# type = "kubernetes"
# gateway_class = "edgion"                 # 必填：匹配的 GatewayClass
# watch_namespaces = []                    # 空 = 所有命名空间
# label_selector = ""                      # 标签过滤
# endpoint_mode = "EndpointSlice"
# gateway_address = ""                     # Gateway status 报告地址
# ha_mode = false                          # 高可用模式
#
# [conf_center.leader_election]
# lease_name = "edgion-controller-leader"
# lease_namespace = "edgion-system"
# lease_duration_seconds = 15
# renew_deadline_seconds = 10
# retry_period_seconds = 2
#
# [conf_center.metadata_filter]
# label_selector = ""                      # 资源标签过滤
# field_selector = ""                      # 资源字段过滤
```

## 字段详解

### [server]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `grpc_listen` | `String` | `0.0.0.0:50051` | ConfigSyncServer gRPC 地址 |
| `admin_listen` | `String` | `0.0.0.0:5800` | Admin API HTTP 地址 |

### [logging]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `log_dir` | `String` | `logs` | 日志目录（相对于 work_dir） |
| `log_prefix` | `String` | `edgion-controller` | 日志文件前缀 |
| `log_level` | `String` | `info` | 日志级别，支持模块级别过滤 |
| `json_format` | `bool` | `false` | JSON 结构化输出 |
| `console` | `bool` | `true` | 同时输出到控制台 |
| `buffer_size` | `usize` | `10000` | 异步日志缓冲区大小 |

### [validation]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enable_reference_grant_validation` | `bool` | `true` | 启用 ReferenceGrant 校验，关闭后跨命名空间引用不受限 |

### [conf_sync]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `default_capacity` | `u32` | `200` | EventStore 默认容量 |
| `capacity_overrides` | `Map<String, u32>` | `{}` | 按 kind 覆盖容量 |
| `no_sync_kinds` | `Vec<String>?` | `["ReferenceGrant", "Secret"]` | 不同步到 Gateway 的资源列表（设置后完全替换默认） |

### [conf_center] — FileSystem

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `type` | `String` | — | `"filesystem"` |
| `conf_dir` | `PathBuf` | — | 资源配置目录 |
| `endpoint_mode` | `String` | `EndpointSlice` | 后端发现：`EndpointSlice` / `Endpoints` / `Both` |

### [conf_center] — Kubernetes

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `type` | `String` | — | `"kubernetes"` |
| `gateway_class` | `String` | **必填** | 匹配的 GatewayClass 名称 |
| `watch_namespaces` | `Vec<String>` | `[]`（全部） | Watch 的命名空间列表 |
| `label_selector` | `String` | `""` | 资源标签过滤 |
| `endpoint_mode` | `String` | `EndpointSlice` | 后端发现模式 |
| `gateway_address` | `String?` | — | Gateway status 中报告的地址 |
| `ha_mode` | `bool` | `false` | 启用 HA 模式（Leader 选举） |

### [conf_center.leader_election]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `lease_name` | `String` | `edgion-controller-leader` | K8s Lease 名称 |
| `lease_namespace` | `String` | `edgion-system` | K8s Lease 命名空间 |
| `lease_duration_seconds` | `u64` | `15` | 租约持续秒数 |
| `renew_deadline_seconds` | `u64` | `10` | 续约截止秒数 |
| `retry_period_seconds` | `u64` | `2` | 竞选重试间隔秒数 |

详细配置参考见 [references/config-reference-controller.md](references/config-reference-controller.md)。
