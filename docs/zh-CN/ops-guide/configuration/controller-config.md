# Controller 配置

edgion-controller 通过 YAML 文件启动，默认路径为 `config/edgion-controller.yaml`，可通过 `--config-file` 指定。

## 完整示例

```yaml
# 工作目录——所有相对路径均基于此目录
# 优先级：CLI --work-dir > 环境变量 EDGION_WORK_DIR > 此值 > "."
work_dir: "."

server:
  grpc_listen: "0.0.0.0:50051"   # ConfigSyncServer gRPC 监听地址
  admin_listen: "0.0.0.0:5800"   # Admin HTTP API 地址

logging:
  log_dir: logs                   # 系统日志目录（相对于 work_dir）
  log_prefix: edgion-controller   # 日志文件名前缀
  log_level: info                 # 级别：trace | debug | info | warn | error
                                  # 支持模块级：info,edgion::core::controller=debug
  json_format: false              # 是否以 JSON 格式输出日志
  console: true                   # 是否同时输出到控制台
  buffer_size: 10000              # 异步日志缓冲区大小

debug:
  enabled: true                   # 启用调试功能

validation:
  enable_reference_grant_validation: true  # 启用 ReferenceGrant 跨命名空间校验

conf_sync:
  default_capacity: 200           # EventStore 默认容量（所有资源类型）
  capacity_overrides:
    # 按资源 kind 单独设置容量，例如：
    # HTTPRoute: 500
    # Service: 1000

  # no_sync_kinds: [ReferenceGrant, Secret]
  # 不同步到 Gateway 的资源列表（设置后完全替换默认列表，不是追加）

# ─── gRPC 通道安全（可选 mTLS）────────────────────────────────────
# conf_sync_security:
#   tls:
#     cert: certs/grpc/server.crt   # Controller 服务端证书（PEM）
#     key: certs/grpc/server.key    # 服务端私钥（PEM）
#     ca: certs/grpc/ca.crt         # 验证 Gateway 客户端证书的 CA
#     skip_tls: false               # true = 紧急降级明文，会输出 ERROR 日志

# ─── conf_center：FileSystem 模式（本地开发）─────────────────────
conf_center:
  type: filesystem
  conf_dir: config/resources      # YAML 资源目录（建议写绝对路径）
  endpoint_mode: EndpointSlice    # EndpointSlice | Endpoints | Both

# ─── conf_center：Kubernetes 模式（生产部署）─────────────────────
# conf_center:
#   type: kubernetes
#   gateway_class: edgion                          # 必填：匹配的 GatewayClass 名称
#   controller_name: edgion.io/gateway-controller  # Gateway API status 回报的 controllerName
#   watch_namespaces: []                           # 空列表 = 监听所有命名空间
#   label_selector: ""                             # 标签过滤
#   endpoint_mode: EndpointSlice
#   gateway_address: ""                            # Gateway status 中报告的地址
#   ha_mode: leader-only                           # "leader-only" | "all-serve"
#   no_watch_kinds: [TCPRoute, UDPRoute]           # 跳过 CRD 预检和 watch 的资源 kind
#   leader_election:
#     lease_name: edgion-controller-leader
#     lease_namespace: edgion-system
#     lease_duration_seconds: 15
#     renew_deadline_seconds: 10
#     retry_period_seconds: 2
#   metadata_filter:
#     blocked_annotations: []     # 进入内存前移除的注解（默认移除 kubectl/helm 常见大注解）
#     remove_managed_fields: true # 减少对象体积
```

## 配置参数

### [server]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `grpc_listen` | `String` | `0.0.0.0:50051` | ConfigSyncServer gRPC 监听地址，Gateway 连接此地址同步配置 |
| `admin_listen` | `String` | `0.0.0.0:5800` | Admin HTTP API 监听地址 |

### [logging]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `log_dir` | `String` | `logs` | 日志目录，相对于 `work_dir` 解析 |
| `log_prefix` | `String` | `edgion-controller` | 日志文件名前缀 |
| `log_level` | `String` | `info` | 日志级别：`trace`、`debug`、`info`、`warn`、`error`，支持模块级过滤（如 `info,edgion::core::controller=debug`） |
| `json_format` | `Boolean` | `false` | 是否以 JSON 结构化格式输出日志 |
| `console` | `Boolean` | `true` | 是否同时输出到控制台 |
| `buffer_size` | `Integer` | `10000` | 异步日志缓冲区大小 |

### [debug]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `Boolean` | `true` | 启用调试功能 |

### [validation]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_reference_grant_validation` | `Boolean` | `true` | 启用 ReferenceGrant 跨命名空间引用校验，关闭后跨命名空间引用不受限制 |

### [conf_sync]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `default_capacity` | `Integer` | `200` | EventStore 对所有资源类型的默认容量 |
| `capacity_overrides` | `Map<String, Integer>` | `{}` | 按资源 kind 单独覆盖容量，例如 `HTTPRoute = 500` |
| `no_sync_kinds` | `Array<String>` | `["ReferenceGrant", "Secret"]` | 不同步到 Gateway 的资源列表，**设置后完全替换默认列表，不是追加**，可选 |

### [conf_sync_security.tls]

可选配置。启用后 Controller 的 gRPC 端口使用 mTLS；不配置此 section 时，gRPC 以明文运行。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `cert` | `String` | — | 服务端证书路径（PEM，相对 work_dir） |
| `key` | `String` | — | 服务端私钥路径（PEM） |
| `ca` | `String` | — | CA 证书路径，用于验证 Gateway 客户端证书 |
| `skip_tls` | `Boolean` | `false` | 紧急降级开关，`true` 时跳过 TLS 以明文启动，会输出 ERROR 日志 |

证书可通过 `edgion-cli gen-certs --output file --output-dir <dir>` 生成。

### [conf_center] — FileSystem 模式

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | `String` | — | 固定填 `"filesystem"` |
| `conf_dir` | `Path` | — | YAML 资源目录，**建议写绝对路径**（当前实现不会随 `work_dir` 自动解析） |
| `endpoint_mode` | `String` | `EndpointSlice` | 后端发现模式：`EndpointSlice` / `Endpoints` / `Both` |

注意：`--conf-dir` CLI 参数会直接覆盖此处的 `conf_dir`，并将 `conf_center` 切换到 FileSystem 模式。

### [conf_center] — Kubernetes 模式

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | `String` | — | 固定填 `"kubernetes"` |
| `gateway_class` | `String` | **必填** | 匹配的 GatewayClass 名称 |
| `controller_name` | `String` | — | Gateway API status 里回报的 `controllerName` |
| `watch_namespaces` | `Array<String>` | `[]`（全部） | 监听的命名空间列表，空列表表示监听所有命名空间 |
| `label_selector` | `String` | `""` | 资源标签过滤表达式 |
| `endpoint_mode` | `String` | `EndpointSlice` | 后端发现模式 |
| `gateway_address` | `String` | — | Gateway status 中报告的地址，可选 |
| `ha_mode` | `String` | — | HA 模式：`"leader-only"` 或 `"all-serve"` |
| `no_watch_kinds` | `Array<String>` | `[]` | 跳过 CRD 预检和 watch 的资源 kind（PascalCase），如 `["TCPRoute", "UDPRoute"]` |

### [conf_center.leader_election]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `lease_name` | `String` | `edgion-controller-leader` | Kubernetes Lease 资源名称 |
| `lease_namespace` | `String` | `edgion-system` | Kubernetes Lease 所在命名空间 |
| `lease_duration_seconds` | `Integer` | `15` | 租约持续秒数 |
| `renew_deadline_seconds` | `Integer` | `10` | 续约截止秒数 |
| `retry_period_seconds` | `Integer` | `2` | 竞选重试间隔秒数 |

### [conf_center.metadata_filter]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `blocked_annotations` | `Array<String>` | kubectl/helm 常见大注解 | 资源进入内存前移除的注解列表 |
| `remove_managed_fields` | `Boolean` | `true` | 移除 `managedFields`，减小对象体积 |

## 常见配置场景

### 本地开发切 FileSystem 模式

使用本地 YAML 文件模拟 Kubernetes 资源，无需集群：

```yaml
conf_center:
  type: filesystem
  conf_dir: /home/user/my-project/config/resources
  endpoint_mode: EndpointSlice
```

也可以在启动时用 CLI 参数覆盖：

```bash
./edgion-controller --config-file config/edgion-controller.yaml \
  --conf-dir /home/user/my-project/config/resources
```

### Kubernetes 生产部署

```yaml
conf_center:
  type: kubernetes
  gateway_class: edgion
  controller_name: edgion.io/gateway-controller
  watch_namespaces: [default, production]
  endpoint_mode: EndpointSlice
  gateway_address: "203.0.113.10"
  ha_mode: leader-only
```

### 调整日志级别

临时提高某个模块的日志详细度，用于排查问题：

```yaml
logging:
  log_level: "info,edgion::core::controller=debug"
  json_format: true  # 方便结构化搜索
```

### HA 多副本部署

在 Kubernetes 多副本场景下启用 Leader Election：

```yaml
conf_center:
  type: kubernetes
  gateway_class: edgion
  ha_mode: leader-only
  leader_election:
    lease_name: edgion-controller-leader
    lease_namespace: edgion-system
    lease_duration_seconds: 15
    renew_deadline_seconds: 10
    retry_period_seconds: 2
```

### 启用 mTLS

确保 Controller 与 Gateway 之间的 gRPC 通信加密：

```yaml
conf_sync_security:
  tls:
    cert: certs/grpc/server.crt
    key: certs/grpc/server.key
    ca: certs/grpc/ca.crt
```

## 相关文档

- [Gateway 配置](./gateway-config.md)
- [EdgionGatewayConfig CRD](./edgion-gateway-config.md)
- [工作目录行为](../../dev-guide/work-directory.md)
