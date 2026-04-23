---
name: controller-config
description: edgion-controller YAML 配置完整 Schema。
---

# Controller YAML 配置 Schema

> 文件路径默认：`config/edgion-controller.yaml`，通过 `--config-file` 指定。

## 看哪些文件

- 样例配置：`config/edgion-controller.yaml`
- 配置结构：`src/core/common/config/mod.rs`
- `conf_center` 顶层：`src/core/controller/conf_mgr/conf_center/config.rs`
- FileSystem 模式：`src/core/controller/conf_mgr/conf_center/file_system/config.rs`
- Kubernetes 模式：`src/core/controller/conf_mgr/conf_center/kubernetes/config.rs`

## 完整 Schema

```yaml
# 工作目录（所有相对路径基于此目录）
# 优先级：CLI --work-dir > ENV EDGION_WORK_DIR > 此值 > "."
work_dir: "."

server:
  grpc_listen: "0.0.0.0:50051"    # gRPC 监听地址（ConfigSyncServer）
  admin_listen: "0.0.0.0:5800"    # Admin HTTP API 地址

logging:
  log_dir: logs                    # 日志目录
  log_prefix: edgion-controller    # 日志文件前缀
  log_level: info                  # trace | debug | info | warn | error
                                   # 支持模块级别：info,edgion::core::controller=debug
  json_format: false               # JSON 结构化日志
  console: true                    # 同时输出到控制台
  buffer_size: 10000               # 日志缓冲区大小

debug:
  enabled: true                    # 启用调试功能

validation:
  enable_reference_grant_validation: true   # 启用 ReferenceGrant 跨命名空间校验

conf_sync:
  default_capacity: 200            # EventStore 默认容量（所有资源类型）
  # 特定资源类型容量覆盖
  # 注意：设置 no_sync_kinds 会完全替换默认列表，不是追加
  capacity_overrides:
    # HTTPRoute: 500
    # Service: 1000
  # no_sync_kinds: [ReferenceGrant, Secret]   # 不同步到 Gateway 的资源

# ─── gRPC 通道安全（可选 mTLS）───
# conf_sync_security:
#   tls:
#     cert: certs/grpc/server.crt    # Controller 服务端证书
#     key: certs/grpc/server.key     # 服务端私钥
#     ca: certs/grpc/ca.crt          # 校验 Gateway 客户端的 CA
#     skip_tls: false                # true = 紧急降级明文（会输出 ERROR 日志）

# ─── FileSystem 模式 ───
conf_center:
  type: filesystem
  conf_dir: config/resources              # YAML 资源目录
  endpoint_mode: EndpointSlice            # EndpointSlice | Endpoints | Both

# ─── Kubernetes 模式 ───
# conf_center:
#   type: kubernetes
#   gateway_class: edgion                 # 必填：匹配的 GatewayClass
#   controller_name: edgion.io/gateway-controller  # Gateway API status 回报的 controllerName
#   watch_namespaces: []                  # 空 = 所有命名空间
#   label_selector: ""                    # 标签过滤
#   endpoint_mode: EndpointSlice
#   gateway_address: ""                   # Gateway status 报告地址
#   ha_mode: leader-only                  # "leader-only" | "all-serve"
#   no_watch_kinds: [TCPRoute, UDPRoute]  # 跳过 CRD 预检和 watch 的资源 kind
#   leader_election:
#     lease_name: edgion-controller-leader
#     lease_namespace: edgion-system
#     lease_duration_seconds: 15
#     renew_deadline_seconds: 10
#     retry_period_seconds: 2
#   metadata_filter:
#     label_selector: ""                  # 资源标签过滤
#     field_selector: ""                  # 资源字段过滤
#     blocked_annotations: []             # 进入内存前移除的注解（默认移除 kubectl/helm 常见大注解）
#     remove_managed_fields: true         # 减少对象体积
```

## 字段详解

### [server]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `grpc_listen` | `String` | `0.0.0.0:50051` | ConfigSyncServer gRPC 地址 |
| `admin_listen` | `String` | `0.0.0.0:5800` | Admin API HTTP 地址 |

> 注意：实际日常开发里默认会加载 `config/edgion-controller.yaml`，所以你通常看到的是 `5800`，不是代码 fallback 的 `8080`。

### [logging]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `log_dir` | `String` | `logs` | 日志目录（相对于 work_dir） |
| `log_prefix` | `String` | `edgion-controller` | 日志文件前缀 |
| `log_level` | `String` | `info` | 日志级别，支持模块级别过滤 |
| `json_format` | `bool` | `false` | JSON 结构化输出 |
| `console` | `bool` | `true` | 同时输出到控制台 |
| `buffer_size` | `usize` | `10000` | 异步日志缓冲区大小 |

> 注意：`log_dir` 会通过 `work_dir().resolve()` 解析，所以相对路径通常会落到 `work_dir/logs/...`。

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

### [conf_sync_security.tls]

可选配置，启用后 Controller 的 gRPC 端口使用 mTLS。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `cert` | `String` | — | 服务端证书路径（PEM，相对 work_dir） |
| `key` | `String` | — | 服务端私钥路径（PEM） |
| `ca` | `String` | — | CA 证书路径（用于验证 Gateway 客户端证书） |
| `skip_tls` | `bool` | `false` | 紧急降级开关：`true` 跳过 TLS，以明文启动（输出 ERROR 日志） |

> 不配置此 section 时，gRPC 以明文运行，与历史版本完全兼容。
> 证书通过 `edgion-cli gen-certs --output file --output-dir <dir>` 生成。

### [conf_center] — FileSystem

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `type` | `String` | — | `"filesystem"` |
| `conf_dir` | `PathBuf` | — | 资源配置目录 |
| `endpoint_mode` | `String` | `EndpointSlice` | 后端发现：`EndpointSlice` / `Endpoints` / `Both` |

重要现实约束：
- `conf_dir` 当前由 `FileSystemStorage::new()` 直接按进程 cwd 解析
- 也就是说，它并不像很多日志路径那样自动跟 `work_dir` 走
- 想避免歧义时，优先写绝对路径

CLI 特例：
- `--conf-dir` 会直接覆盖这里的 `conf_dir`
- 而且会把 `conf_center` 切到 FileSystem 变体，只保留原来的 `endpoint_mode`

### [conf_center] — Kubernetes

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `type` | `String` | — | `"kubernetes"` |
| `gateway_class` | `String` | **必填** | 匹配的 GatewayClass 名称 |
| `controller_name` | `String` | — | Gateway API status 里回报的 controllerName |
| `watch_namespaces` | `Vec<String>` | `[]`（全部） | Watch 的命名空间列表 |
| `label_selector` | `String` | `""` | 资源标签过滤 |
| `endpoint_mode` | `String` | `EndpointSlice` | 后端发现模式 |
| `gateway_address` | `String?` | — | Gateway status 中报告的地址 |
| `ha_mode` | `String` | — | `"leader-only"` 或 `"all-serve"` |
| `no_watch_kinds` | `Vec<String>` | `[]` | 不探测/不 watch 的资源 kind（PascalCase），如 `["TCPRoute", "UDPRoute", "BackendTLSPolicy"]`。启动时跳过 CRD 预检，不 spawn watcher，从 processor readiness 中排除 |

### [conf_center.leader_election]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `lease_name` | `String` | `edgion-controller-leader` | K8s Lease 名称 |
| `lease_namespace` | `String` | `edgion-system` | K8s Lease 命名空间 |
| `lease_duration_seconds` | `u64` | `15` | 租约持续秒数 |
| `renew_deadline_seconds` | `u64` | `10` | 续约截止秒数 |
| `retry_period_seconds` | `u64` | `2` | 竞选重试间隔秒数 |

### [conf_center.metadata_filter]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `blocked_annotations` | `Vec<String>` | kubectl/helm 常见大注解 | 进入内存前移除的注解 |
| `remove_managed_fields` | `bool` | `true` | 减少对象体积 |

## `[auth]` — OIDC 认证（Admin API）

启用后 Admin API 需要 OIDC JWT Bearer token。定义在 `src/core/common/auth/config.rs::AdminAuthConfig`，由 `UnifiedAuthState::from_configs` 装进统一认证状态，最终通过 `common::api::compose_admin_routes` 挂载 `unified_auth_middleware` 生效。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | `bool` | `true`（存在 section 即启用） | 启用 OIDC |
| `discovery` | `String` | — | **必填**。OIDC discovery URL（`/.well-known/openid-configuration`） |
| `audiences` | `Vec<String>` | `[]` | JWT `aud` claim 期望值；空数组跳过校验 |
| `issuers` | `Vec<String>` | `[]` | JWT `iss` 期望值；空数组时与 discovery document 的 issuer 校验 |
| `allowed_algorithms` | `Vec<String>` | `[RS256, RS384, RS512, ES256, ES384]` | 允许的签名算法 |
| `clock_skew_seconds` | `u64` | `120` | JWT 时间戳容差（秒） |
| `jwks_cache_ttl` | `u64` | `300` | JWKS 缓存 TTL（秒） |
| `jwks_min_refresh_interval` | `u64` | `10` | JWKS 刷新最小间隔（秒），防抖动 |
| `skip_paths` | `Vec<String>` | `["/health", "/ready"]` | 跳过认证的路径前缀 |
| `ssl_verify` | `bool` | `true` | 拉 discovery/JWKS 时是否校验 TLS 证书；生产必须 `true` |
| `discovery_max_response_bytes` | `usize` | `65536` (64 KB) | OIDC discovery 响应体上限；防 DoS，合规 IdP 典型 < 4 KB |
| `jwks_max_response_bytes` | `usize` | `1048576` (1 MB) | JWKS 响应体上限；典型 JWKS < 10 KB |

```yaml
auth:
  discovery: "https://keycloak.example.com/realms/edgion/.well-known/openid-configuration"
  audiences: [edgion-admin]
  # 默认值足够绝大多数场景，下面两行通常不用配
  # discovery_max_response_bytes: 65536
  # jwks_max_response_bytes: 1048576
```

> `[auth]` 与 `[local_auth]` 可同时存在；请求按 unified_auth 中间件顺序尝试（OIDC → local）。详见 `skills/01-architecture/01-controller/02-admin-api.md`。

## `[local_auth]` — 本地认证

内置用户名/密码认证，签发 HS256 JWT。**Controller** 不配置时自动生成随机凭据存入 Secret
（Center 行为不同，见下方引用）。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `bool` | `true` | 启用本地认证 |
| `username` | `String` | `"admin"` | 用户名 |
| `password` | `String` | `""` | 密码（空 = 需自动生成）|
| `jwt_secret` | `String` | `""` | JWT 签名密钥（空 = 需自动生成）|
| `jwt_expiry_hours` | `u64` | `24` | Token 过期时间（小时）|
| `skip_paths` | `Vec<String>` | `["/health", "/ready"]` | 跳过认证的路径前缀 |

```yaml
local_auth:
  enabled: true
  username: admin
  password: my-secure-password
  jwt_secret: my-jwt-signing-secret
  jwt_expiry_hours: 24
```

### 自动生成凭据（仅 Controller）

当 Controller 无 `[auth]`（OIDC）且无 `[local_auth]` 配置时：

1. Controller 启动时通过 CenterApi 检查 Secret `edgion-admin-credentials`（namespace: `edgion-system`）
2. 不存在 → 生成随机 20 位密码 + 32 位 JWT secret → 创建 Secret
3. 已存在 → 读取凭据继续启动
4. FileSystem 模式：`{conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml`
5. K8s 模式：通过 K8s API 创建真正的 Secret

> **Center 行为不同**：Center 没有 Secret 自动生成路径。无 `[local_auth]` + 无 `[auth]`
> 时 Center 启动，但所有业务路由以 503 拒绝（fail-close via
> `unified_auth_middleware` 的 initializing 分支）。详见
> `skills/02-features/02-config/03-auth-bootstrap.md`。

## 当前项目里最常见的 Controller 配置改动

- 本地测试切换配置目录：改 `conf_center.type = "file_system"` + `conf_dir`
- K8s 部署只管某个 GatewayClass：改 `gateway_class`
- 控制不同 kind 的缓存上限：改 `conf_sync.capacity_overrides`
- 调整观测噪音：改 `logging.log_level`
