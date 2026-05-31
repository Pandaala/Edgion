# Controller 配置

edgion-controller 从 YAML 文件读取配置，默认路径为 `config/edgion-controller.yaml`，可通过 `--config-file` 指定。

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
  type: file_system
  conf_dir: config/resources      # YAML 资源目录（建议写绝对路径）

# ─── conf_center：Kubernetes 模式（生产部署）─────────────────────
# conf_center:
#   type: kubernetes
#   gateway_class: edgion                          # 必填：匹配的 GatewayClass 名称
#   controller_name: edgion.io/gateway-controller  # Gateway API status 回报的 controllerName
#   watch_namespaces: []                           # 空列表 = 监听所有命名空间
#   label_selector: ""                             # 标签过滤
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
| `type` | `String` | — | 必须填 `file_system`（snake_case，不是 `filesystem`） |
| `conf_dir` | `Path` | — | YAML 资源目录，**建议写绝对路径**（当前实现不会随 `work_dir` 自动解析） |

注意：`--conf-dir` CLI 参数会直接覆盖此处的 `conf_dir`，并将 `conf_center` 切换到 FileSystem 模式。

### [conf_center] — Kubernetes 模式

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | `String` | — | 固定填 `"kubernetes"` |
| `gateway_class` | `String` | **必填** | 匹配的 GatewayClass 名称 |
| `controller_name` | `String` | — | Gateway API status 里回报的 `controllerName` |
| `watch_namespaces` | `Array<String>` | `[]`（全部） | 监听的命名空间列表，空列表表示监听所有命名空间 |
| `label_selector` | `String` | `""` | 资源标签过滤表达式 |
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
| `label_selector` | `String` | `""` | 应用于被监控资源的标签选择器 |
| `field_selector` | `String` | `""` | 应用于被监控资源的字段选择器 |
| `blocked_annotations` | `Array<String>` | kubectl/helm 常见大注解 | 资源进入内存前移除的注解列表 |
| `remove_managed_fields` | `Boolean` | `true` | 移除 `managedFields`，减小对象体积 |

## 认证

Controller Admin API 的认证是**必须的**。所有对 `/api/v1/*` 路由的请求都需要有效的 Bearer Token 或会话 Cookie。未认证的请求返回 HTTP 401。

> 零配置也可工作：当 `[local_auth]` 不存在时，Controller 自动生成随机管理员密码和 JWT 密钥，存储在 `edgion-admin-credentials` Secret（`edgion-system` 命名空间）中。获取生成的密码：
>
> **Kubernetes 模式**：
> ```bash
> kubectl -n edgion-system get secret edgion-admin-credentials \
>   -o jsonpath='{.data.password}' | base64 -d
> ```
>
> **本地 / 文件模式**：
> ```bash
> cat {conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml
> ```

Controller 和 Center 使用相同的 `[local_auth]` / `[auth]` schema，但引导方式不同：Controller 在两者均未设置时自动生成凭据，而 Center 则失败关闭（见下方 Center 说明）。

### [local_auth]

内置用户名/密码认证，向已认证客户端签发 HS256 JWT。Controller 上不存在时自动生成凭据（见上文）。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `username` | `String` | `"admin"` | 管理员用户名 |
| `password` | `String` | _（自动生成）_ | 管理员密码。Controller 上为空时自动生成；Center 上必须显式设置 |
| `jwt_secret` | `String` | _（自动生成）_ | JWT 签名密钥。Controller 上为空时自动生成；Center 上必须显式设置 |
| `jwt_expiry_hours` | `Integer` | `24` | 签发 Token 的有效时长（小时） |
| `cookie_secure` | `Boolean` | `true` | 在 `edgion_token` 认证 Cookie 上设置 `Secure` 属性。TLS 前置的部署保持 `true`；仅明文 HTTP 的开发/内部环境设为 `false` |

```yaml
local_auth:
  username: admin
  password: "change-me"
  jwt_secret: "a-long-random-signing-secret"
  jwt_expiry_hours: 24
  cookie_secure: true
```

> **Center 说明**：Center **没有**自动生成。若 `[local_auth]` 和 `[auth]` 均未配置，业务路由返回 503（失败关闭），而 `/health`、`/ready`、`/metrics` 和 `/api/v1/auth/status` 保持可访问。

### [auth] — OIDC

可选的 Admin API OIDC 集成。根据 OIDC 提供商的 discovery 文档和 JWKS 验证 JWT Bearer Token。可与 `[local_auth]` 共存；请求按 OIDC → local 顺序尝试。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `Boolean` | `true`（段存在时） | 启用 OIDC 验证 |
| `discovery` | `String` | **必填** | OIDC discovery URL（`/.well-known/openid-configuration`） |
| `audiences` | `Array<String>` | `[]` | 期望的 JWT `aud` 值；为空时跳过 audience 检查 |
| `issuers` | `Array<String>` | `[]` | 期望的 JWT `iss` 值；为空时根据 discovery 文档的 issuer 验证 |
| `allowed_algorithms` | `Array<String>` | `[RS256, RS384, RS512, ES256, ES384]` | 允许的签名算法 |
| `clock_skew_seconds` | `Integer` | `120` | JWT 时间戳容差（秒） |
| `jwks_cache_ttl` | `Integer` | `300` | JWKS 缓存 TTL（秒） |
| `jwks_min_refresh_interval` | `Integer` | `10` | JWKS 刷新尝试的最小间隔（防抖），秒 |
| `skip_paths` | `Array<String>` | `["/health", "/ready"]` | 跳过认证的路径前缀 |
| `ssl_verify` | `Boolean` | `true` | 获取 discovery/JWKS 时验证 TLS 证书；生产环境必须保持 `true` |
| `discovery_max_response_bytes` | `Integer` | `65536` | discovery 响应体大小上限 |
| `jwks_max_response_bytes` | `Integer` | `1048576` | JWKS 响应体大小上限 |

```yaml
auth:
  discovery: "https://keycloak.example.com/realms/edgion/.well-known/openid-configuration"
  audiences: [edgion-admin]
```

### [cli_tokens] — 静态 CLI Token

用于编程或 CI 访问 Admin API 的静态 Bearer Token。每个 Token 绑定到细粒度的 RBAC 策略。Token 以 SHA-256 十六进制摘要存储（永不明文）。支持热重载。

| 参数 | 类型 | 说明 |
|------|------|------|
| `name` | `String` | 此 Token 的人类可读标签 |
| `token_sha256` | `String` | 原始 Bearer Token 的 SHA-256 十六进制摘要（64 个小写十六进制字符）。使用 `edgion-cli gen-token` 生成 |
| `rbac` | Object | RBAC 允许规则（`verbs × kinds`）。缺失或为空则拒绝所有 |

```yaml
cli_tokens:
  - name: "ci-readonly"
    token_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
    rbac:
      allow:
        # 枚举具体 kind；切勿对读取动词使用 kinds: ["*"]
        # （通配符读取会暴露 Secret 和 EdgionTls 的私钥材料）
        - verbs: [get, list, watch]
          kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service, EdgionGatewayConfig]
  - name: "deploy-bot"
    token_sha256: "1111111111111111111111111111111111111111111111111111111111111111"
    rbac:
      allow:
        - verbs: [get, list, create, update, delete]
          kinds: [HTTPRoute, Service]
```

Token 生成工作流、轮换说明和所有安全注意事项，详见[联邦 RBAC — CLI Token RBAC](../federation-rbac.md#cli-token-rbac)。

## 联邦客户端（center）

添加可选的 `center` 段以将此 Controller 加入联邦 Center。不使用联邦时完全省略。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `address` | `String` | **必填** | Center 服务器的 gRPC 地址（如 `https://center.example.com:50052`） |
| `name` | `String` | **必填** | 此 Controller 的逻辑名称，上报给 Center |
| `cluster` | `String` | **必填** | 集群名称，用于路由和聚合 |
| `enabled` | `Boolean` | `true` | 热重载开关；`false` 可停止联邦客户端而无需重启（通过 `POST /api/v1/reload` 重载） |
| `env` | `Array<String>` | `[]` | 上报给 Center 的环境标签 |
| `tag` | `Array<String>` | `[]` | 上报给 Center 的扩展标签 |
| `ping_interval_secs` | `Integer` | `30` | 联邦 keepalive ping 间隔 |
| `security` | Object | — | 通道的 mTLS 证书；必填（无此配置时联邦失败关闭） |
| `rbac` | Object | 内置默认 | Center 操作的 RBAC；缺失时应用内置默认策略 |

```yaml
center:
  address: "https://center.example.com:50052"
  name: "ctrl-1"
  cluster: "prod"
  enabled: true
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
```

完整的 `center.rbac` 模型、默认策略和安全说明，详见[联邦 RBAC](../federation-rbac.md)。

## 常见配置场景

### 本地开发切 FileSystem 模式

使用本地 YAML 文件模拟 Kubernetes 资源，无需集群：

```yaml
conf_center:
  type: file_system
  conf_dir: /home/user/my-project/config/resources
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
- [联邦 RBAC](../federation-rbac.md)
