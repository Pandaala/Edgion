# Controller 配置

edgion-controller 从 YAML 文件读取配置，默认路径为 `config/edgion-controller.yaml`，可通过 `--config-file` 指定。

## 完整示例

```yaml
# 工作目录——所有相对路径均基于此目录
# 优先级：CLI --work-dir > 环境变量 EDGION_WORK_DIR > 此值 > "."
work_dir: "."

server:
  grpc_listen: "0.0.0.0:12151"   # ConfigSyncServer gRPC 监听地址
  admin_listen: "127.0.0.1:12101"  # Admin HTTP API 地址（/api/v1、/configclient）——默认回环地址；集群/远程访问需设置 0.0.0.0 并配置 allow_admin_ips
  # probe_listen: "0.0.0.0:12100"   # 存活/就绪探针（/health、/ready）；默认 0.0.0.0:12100
  # metrics_listen: "0.0.0.0:12190" # Prometheus 指标（/metrics）；默认 0.0.0.0:12190

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

# ─── gRPC 通道安全（必填）─────────────────────────────────────────
# 省略此段会导致 Controller 拒绝启动。必须从下面三个选项中选择其一
# （auto_ca / 证书路径 / skip_tls）。
#
# 选项 1 —— Kubernetes：由 Controller 内置 CA 自动签发。
conf_sync_security:
  auto_ca: true
#
# 选项 2 —— 文件模式 mTLS：
# conf_sync_security:
#   tls:
#     cert: certs/grpc/server.crt   # Controller 服务端证书（PEM）
#     key: certs/grpc/server.key    # 服务端私钥（PEM）
#     ca: certs/grpc/ca.crt         # 验证 Gateway 客户端证书的 CA
#
# 选项 3 —— 显式选择明文（仅限本地开发 / 非生产，会输出 ERROR 日志）：
# conf_sync_security:
#   tls:
#     skip_tls: true

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
| `grpc_listen` | `String` | `0.0.0.0:12151` | ConfigSyncServer gRPC 监听地址，Gateway 连接此地址同步配置 |
| `admin_listen` | `String` | `127.0.0.1:12101` | Admin HTTP API 监听地址，提供 `/api/v1/*` 和 `/configclient`。默认绑定回环地址；如需集群或远程访问，需将其设置为 `0.0.0.0` 并配置 `allow_admin_ips` |
| `probe_listen` | `String` | `0.0.0.0:12100` | 探针监听地址，提供 `/health`（存活）和 `/ready`（就绪）；Kubernetes pod spec 中使用 `httpGet.port: 12100` |
| `metrics_listen` | `String` | `0.0.0.0:12190` | 指标监听地址，提供 `/metrics`（Prometheus 抓取目标） |

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

### [conf_sync_security]

**必填配置。** conf_sync gRPC 通道会把解析后的 Secret（TLS 私钥、后端凭证）下发到数据面，
因此其传输方式必须由运维显式选择。

**完全省略 `conf_sync_security` 段会导致 Controller 拒绝启动**（非零退出码，并打印可操作的错误信息）。
必须且只能配置以下之一：

| 配置状态 | 结果 |
|---------|------|
| `auto_ca: true` | mTLS —— 由 Controller 内置 CA 自动签发 CA 与 Gateway 客户端证书（Kubernetes 推荐） |
| `certs` / 旧格式 `tls` 段，含 `cert` + `key` + `ca` | 文件模式 mTLS |
| `tls.skip_tls: true` | 显式选择明文（输出 ERROR 日志；仅限本地开发 / 非生产） |
| *（省略该段）* | **拒绝启动** |

> **迁移提示**：过去省略该段意味着"明文 gRPC"，依赖此行为的部署现在将无法启动。
> 若要继续使用明文，请显式声明 `conf_sync_security: {tls: {skip_tls: true}}`；
> 否则请配置 `auto_ca` 或证书路径。

#### [conf_sync_security.tls]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `cert` | `String` | — | 服务端证书路径（PEM，相对 work_dir） |
| `key` | `String` | — | 服务端私钥路径（PEM） |
| `ca` | `String` | — | CA 证书路径，用于验证 Gateway 客户端证书 |
| `skip_tls` | `Boolean` | `false` | 显式明文开关，`true` 时跳过 TLS 以明文启动，会输出 ERROR 日志并置位 `edgion_system_error_state{reason="skip_tls"}` |

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

Controller Admin API 采用 **Istio 风格的纯 Token 认证**。每个对 `/api/v1/*` 业务路由的请求都必须携带 `Authorization: Bearer <token>` 头。**不再有**浏览器登录、会话 Cookie 或 Admin API 的 OIDC——这些已被移除。（浏览器仪表板现已迁移到独立的 Center 项目；Controller 不再托管仪表板。）

单个预共享的**管理员 Token** 授予完整（allow-all）访问权限。其来源如下（优先级从高到低）：

1. 环境变量 `EDGION_ADMIN_TOKEN`，通过 `valueFrom.secretKeyRef` 从 `edgion-admin-credentials` Secret（键 `token`）注入——Kubernetes 推荐方式。`deploy.sh` 脚本会幂等地创建该 Secret。
2. 顶层 `admin_token` 配置字段（见下文）。

空或缺失的 Token 表示"未配置"。当既未设置管理员 Token **又**未配置 `cli_tokens` 时，服务器仍可启动，但所有 Admin API 业务路由返回 **503 fail-close**。`/health`、`/ready`、`/metrics` 和 `GET /api/v1/auth/status` 保持可访问。

从 Secret 读取 Token（使用 `deploy.sh` 部署时）：

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

用它授权 CLI：

```bash
edgion-cli login --token <TOKEN>
```

### 授权模型

Token 映射到一个 RBAC 策略。**本地管理员 Token** 映射到 allow-all（通配符）策略——可对任意 kind 执行任意操作。细粒度的按 Token RBAC 通过 `cli_tokens`（见下文）和联邦（`center.rbac`）提供，两者均保持不变。除 allow-all 管理员 Token 外，不再有独立的"superuser"概念。

认证为 **纯 Bearer**——没有 httpOnly Cookie，也没有服务端会话。

### 认证端点

`GET /api/v1/auth/status` 是唯一的认证端点，始终公开（无需 Token）。返回：

```json
{ "auth_required": true, "mode": "token", "status": "..." }
```

原 `/api/v1/auth/login`、`/api/v1/auth/logout` 和 `/api/v1/auth/me` 端点已被移除。

### `admin_token`

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `admin_token` | `String` | _（空）_ | 授予 allow-all 访问权限的预共享管理员 Bearer Token。空 = 未配置。优先通过 `EDGION_ADMIN_TOKEN` 环境变量从 Secret 注入，而非在配置文件中以明文写入。 |

```yaml
# 顶层配置字段。优先通过 EDGION_ADMIN_TOKEN 环境变量从 Secret 注入，
# 而非将 Token 提交到明文配置文件。
admin_token: "edg_..."
```

> **说明**：Center 保留自己的认证引导方式（OIDC / `local_auth`），不受此纯 Token 模型影响；仅 Controller Admin API 发生了变化。加入 Center 的方式见[联邦客户端](#联邦客户端center)章节。

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
| `address` | `String` | **必填** | Center 服务器的 gRPC 地址（如 `https://center.example.com:12251`） |
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
  address: "https://center.example.com:12251"
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
