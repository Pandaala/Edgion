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
#   active: primary
#   certs:
#     - name: primary
#       cert: certs/grpc/server.crt   # Controller 服务端证书（PEM）
#       key: certs/grpc/server.key    # 服务端私钥（PEM）
#       ca: certs/grpc/ca.crt         # 验证 Gateway 客户端证书的 CA
#
# 选项 3 —— 显式选择明文（仅限本地开发 / 非生产，会输出 ERROR 日志）：
# conf_sync_security:
#   active: insecure
#   certs:
#     - name: insecure
#       skip_tls: true

# ─── conf_center：FileSystem 模式（本地开发）─────────────────────
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: config/resources      # YAML 资源目录（建议写绝对路径）

# ─── conf_center：Kubernetes 模式（生产部署）─────────────────────
# conf_center:
#   type: kubernetes
#   controller_name: edgion.io/gateway-controller  # Gateway API status 回报的 controllerName
#   watch_namespaces: []                           # 空列表 = 监听所有命名空间
#   label_selector: ""                             # 标签过滤
#   gateway_address: ""                            # Gateway status 中报告的地址
#   ha_mode: leader-only                           # "leader-only" | "all-serve"
#   no_watch_kinds: [TCPRoute, UDPRoute]           # 跳过 CRD 预检和 watch 的资源 kind
#   cache_ready_deadline_secs: 90                  # CachesReady 前的超时上限（秒）；随进程重启生效，不支持热重载
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

List 和 Watch 载荷超过 32 MiB 时，如果压缩能减小体积，会自动采用 Zstd level 3。
Gateway 解压后再应用配置。原有的 64 MiB List 限制已移除；两种操作均采用解压后
512 MiB 的传输安全预算。超限返回 `ResourceExhausted`，不会截断配置。压缩减少网络
传输量，不会消除解压后配置所需的内存。升级时 Controller 和 Gateway 必须使用相同
的载荷协议版本，无需配置压缩开关。这些限制不改变 Kubernetes 对象限制或 WAF
规则包校验限制。

**必填配置。** conf_sync gRPC 通道会把解析后的 Secret（TLS 私钥、后端凭证）下发到数据面，
因此其传输方式必须由运维显式选择。

**完全省略 `conf_sync_security` 段会导致 Controller 拒绝启动**（非零退出码，并打印可操作的错误信息）。
必须且只能配置以下之一：

| 配置状态 | 结果 |
|---------|------|
| `auto_ca: true` | mTLS —— 由 Controller 内置 CA 自动签发 CA 与 Gateway 客户端证书（Kubernetes 推荐） |
| 选中的 `certs` 条目，含 `cert` + `key` + `ca` | 文件模式 mTLS |
| `skip_tls: true` | 显式选择明文（输出 ERROR 日志；仅限本地开发 / 非生产） |
| *（省略该段）* | **拒绝启动** |

> **迁移提示**：过去省略该段意味着"明文 gRPC"，依赖此行为的部署现在将无法启动。
> 若要继续使用明文，请显式声明 `conf_sync_security: {certs: [{skip_tls: true}]}`；
> 否则请配置 `auto_ca` 或证书路径。

#### [conf_sync_security.certs]

`certs` 是证书集列表，`active` 按 `name` 选择条目；仅有一个条目时可以省略。
重复名称和未匹配的选择器会被拒绝。旧的单套 `tls` 配置必须迁移为 `certs` 条目。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | `String?` | — | 条目名称，供 `active` 选择 |
| `cert` | `String` | — | 服务端证书路径（PEM，相对 work_dir） |
| `key` | `String` | — | 服务端私钥路径（PEM） |
| `ca` | `String` | — | CA 证书路径，用于验证 Gateway 客户端证书 |
| `skip_tls` | `Boolean` | `false` | 显式明文开关，`true` 时跳过 TLS 以明文启动，会输出 ERROR 日志并置位 `edgion_system_error_state{reason="skip_tls"}` |

证书可通过 `edgion-cli gen-certs --output file --output-dir <dir>` 生成。

### Controller 身份

`conf_center.controller_name` 必须与 GatewayClass 的 `spec.controllerName` 完全一致，
采用 Gateway API 要求的域名前缀加路径格式，最多 253 个字符。显式配置为空白或格式非法时启动失败。

FileSystem 和 etcd 必须显式配置该字段。仅 Kubernetes 在省略时使用
`edgion.io/gateway-controller/{POD_NAMESPACE}/{EDGION_CONTROLLER_DEPLOYMENT_NAME}`。
通过 `metadata.namespace` 注入 `POD_NAMESPACE`，显式注入稳定的 Deployment 名称作为
`EDGION_CONTROLLER_DEPLOYMENT_NAME`，不能使用 Pod 名称。组成部分缺失或非法时启动失败。
例如 namespace 为 `edgion-system`、Deployment 为 `edgion-controller` 时，GatewayClass 应配置
`edgion.io/gateway-controller/edgion-system/edgion-controller`。

独立部署必须使用不同的 controller name 和 GatewayClass；运行时不会检测显式身份冲突。
同一身份的副本必须共享配置视图和选举域。独立 Kubernetes 部署还需使用不同的选主 Lease；
自动生成 controller name 不会修改 lease_name 或 lease_namespace。GatewayClass 的 `spec.controllerName`
不可变；更换身份时需要创建替代类。

Controller 接管所有 `spec.controllerName` 与部署身份匹配的 GatewayClass，并逐个执行参数校验。
每个 Gateway 使用自己引用的 class 的 `parametersRef`；新增匹配类无需重启 Controller。
没有类名筛选配置；显式提供已移除的 `conf_center.gateway_class`（包括 null）会导致启动失败。
请通过 RBAC 控制 GatewayClass 创建和 controllerName 分配。多个 class 不支持共用一个物理监听端口。
GatewayClass 的监听忽略 `label_selector`，覆盖没有匹配标签的类。
将 `GatewayClass` 加入 `no_watch_kinds` 会关闭此报告，并停止 Gateway 处理。

### [conf_center] — FileSystem 模式

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | `String` | — | 必须填 `file_system`（snake_case，不是 `filesystem`） |
| `controller_name` | `String` | **必填** | 显式 Gateway API 控制器身份 |
| `conf_dir` | `Path` | — | YAML 资源目录，**建议写绝对路径**（当前实现不会随 `work_dir` 自动解析） |

注意：`--conf-dir` CLI 参数会直接覆盖此处的 `conf_dir`，并将 `conf_center` 切换到 FileSystem 模式。

### [conf_center] — Kubernetes 模式

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | `String` | — | 固定填 `"kubernetes"` |
| `controller_name` | `String` | namespace + Deployment 身份 | 显式覆盖自动生成的控制器身份 |
| `watch_namespaces` | `Array<String>` | `[]`（全部） | 监听的命名空间列表，空列表表示监听所有命名空间 |
| `label_selector` | `String` | `""` | 资源标签过滤表达式，不作用于 GatewayClass |
| `gateway_address` | `String` | — | Gateway status 中报告的地址，可选 |
| `ha_mode` | `String` | — | HA 模式：`"leader-only"` 或 `"all-serve"` |
| `no_watch_kinds` | `Array<String>` | `[]` | 跳过 CRD 预检和 watch 的资源 kind（PascalCase），如 `["TCPRoute", "UDPRoute"]` |
| `cache_ready_deadline_secs` | `Integer` | `90` | 已启动的 watcher 集合到达 `CachesReady` 前的超时上限（秒）——覆盖内部 30 秒的就绪轮询预算，加上就绪后的收尾阶段（WatchObj 注册、revalidation、leader-job 启动）。**必须 `> 30`**，否则启动时校验失败拒绝启动。**随进程重启生效，不支持热重载**——`POST /api/v1/reload` 每次迭代都会重新武装该超时，但始终使用启动时捕获的值；修改此字段需要完整重启进程。经验法则：当单一资源 kind 的数量接近每 kind 工作队列容量（5000）时应调大此值——由于每次超时触发的重启都会 `runtime.reset()` 并从头重放，过小的值会造成**永久性**重启循环而不是一次性失败。最坏情况下，重启触发前的等待时间约为配置值的 **2 倍**（同一超时预算还会通过 `tokio::time::timeout` 包裹最初的 `start_event_watchers` 调用，返回后再为收尾阶段重新武装一次）。完整机制见 [Supervision & recovery](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md#supervision--recovery) |

首次启动时，Controller 会等待所需监听来源完成首次完整加载，再发布配置同步服务。
运行期间 Kubernetes 重新 LIST 时，会保留该来源上一次完整快照，成功后再替换，并通过
原有 worker 处理变化。普通 relist 不会重启全部资源 Controller 或撤下配置同步服务。
Namespace 标签变化也会触发使用 Selector 的 Gateway 重新计算。

管理接口 reload 和不可恢复的 Controller 任务故障仍会在旧任务退出后重建资源状态。
`leader-only` 模式丢失领导权时也会停止当前资源运行周期；`all-serve` 模式的领导权
切换会保留资源监听和配置服务，仅调整领导者专属服务。

增大启动超时可以容纳较慢但仍有进展的处理，不能解决真正的队列容量死锁。
反复初始化超时时，应同时检查阻塞的处理路径和资源数量。

### [conf_center.leader_election]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `lease_name` | `String` | `edgion-controller-leader` | Kubernetes Lease 资源名称 |
| `lease_namespace` | `String` | `edgion-system` | Kubernetes Lease 所在命名空间 |
| `lease_duration_seconds` | `Integer` | `15` | 租约持续秒数 |
| `renew_deadline_seconds` | `Integer` | `10` | 续约截止秒数 |
| `retry_period_seconds` | `Integer` | `2` | 竞选重试间隔秒数 |

### [conf_center] — Etcd 模式

Etcd 模式面向非 Kubernetes（裸金属 / 虚拟机）生产环境：Controller 将资源存储到 etcd 集群并监听变更，
而不使用 Kubernetes API。它通过下方的 `leader_election` 段支持主备（active-passive）HA。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | `String` | — | 固定填 `etcd` |
| `controller_name` | `String` | **必填** | 显式 Gateway API 控制器身份 |
| `endpoints` | `Array<String>` | **必填** | etcd 集群端点列表，如 `["http://10.0.0.1:2379"]` |
| `key_prefix` | `String` | `/edgion` | 存入 etcd 的所有配置键的前缀 |
| `auth` | Object | — | 可选的 etcd 用户名 / 密码认证（`auth.username`、`auth.password`） |
| `tls` | Object | — | 可选，用于 **Controller→etcd** 连接的 TLS（文件路径）：`tls.ca_cert_file`、`tls.client_cert_file`、`tls.client_key_file`。与 conf_sync mTLS 通道无关 |
| `connect_timeout_ms` | `Integer` | `5000` | etcd 连接（拨号）超时，毫秒 |
| `request_timeout_ms` | `Integer` | `10000` | 共享 conf_center etcd 客户端一元调用的单请求超时，毫秒。**注意**：它**不会**作用于阻塞式的 Leader 竞选 `campaign()` RPC（竞选客户端刻意不设置通道级超时），因此运维无需为竞选调整此值 |
| `leader_election` | Object | — | 主备 HA 设置（见下文）。**始终开启**，没有启用 / 禁用开关 |

### [conf_center.leader_election] — Etcd 模式

Etcd 模式的 Leader Election 使用 etcd 官方的 `v3election` gRPC 服务，外加自管理的租约 keepalive。
**这些字段与上方的 Kubernetes `leader_election` 段完全不同**——Etcd 段**没有**
`lease_name`/`lease_namespace`（不存在 K8s Lease 对象），而是**必须**提供 `grpc_addr` 和 `admin_addr`。

恰好一个 Controller 成为 Leader（运行资源控制器并对外提供 conf_sync）；备节点拒绝数据面流量，并在
Admin API 上返回 `409` 且指明 Leader 的 `admin_addr`。单节点部署时唯一的候选者会立即赢得竞选。
Kubernetes 模式（拥有自己基于 Lease 的 HA）不受影响。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `identity` | `String` | 主机名 | 仅用于展示——出现在日志和非 Leader 的 `409` 响应体中。**绝不**用于所有权判定（按租约的键天然区分各届任期）。默认取主机的 `HOSTNAME` |
| `grpc_addr` | `String` | **必填** | 网关连接的 conf_sync gRPC 地址。必须是**可路由的非回环**地址（不能是 `127.0.0.1` / `0.0.0.0` / `localhost`）——它是网关故障转移时轮换的目标 |
| `admin_addr` | `String` | **必填** | 非 Leader `409` 响应体中指明的 Admin HTTP 地址。必须是**可路由的非回环**地址 |
| `lease_duration_secs` | `Integer` | `15` | 会话租约 TTL。双 Leader 窗口与拆除预算都被 `lease_duration − renew_deadline` 界定 |
| `renew_deadline_secs` | `Integer` | `10` | 若在此窗口内无法记录一次成功的 keepalive，Leader 会自行退位 |
| `retry_period_secs` | `Integer` | `2` | keepalive / 守卫事务（guard txn）节奏 |

**启动校验（fail fast）。** 除非满足 `retry_period_secs < renew_deadline_secs < lease_duration_secs`
且三者均 `> 0`，**并且** `floor(renew_deadline_secs / retry_period_secs) >= 3`，否则 Controller 在启动时
拒绝该配置。默认值 `2 / 10 / 15` 满足此约束。

**部署要求与注意事项：**

- **必须提供 `v3election`。** 原生 etcd 默认提供该服务，但某些代理 / 托管的 etcd 部署（以及某些
  `grpc-proxy` 配置）会禁用 `v3election`/`v3lock` gRPC 服务。Controller 在启动时探测它，缺失则以可操作
  的错误信息快速失败。
- **默认值假设 etcd 位于 LAN 内。** 跨 WAN 的 etcd 应调高这三个值，且 etcd 自身的选举超时必须远小于
  `renew_deadline_secs`（校验无法检查这一点）。
- **etcd 模式下 conf_sync mTLS 需手动签发证书。** `auto_ca` 自动签发是 **Kubernetes 专用**的（需要
  kube API 客户端且硬编码 K8s Service DNS SAN），在裸金属 / 虚拟机上会在启动时失败。请使用
  `edgion-cli gen-certs --server-san <name>`（可重复），并确保 conf_sync **服务端**证书的 SAN 覆盖网关
  在故障转移时可能轮换到的**每一个** Controller 的 `grpc_addr`——否则网关重连到新 Leader 时会 TLS 校验
  失败。
- **备节点的 `/ready` 返回 `503`**（它没有 ConfigSyncServer）。**不要**在 Admin/探针端口前放置会杀死健康
  备节点的 TCP/HTTP 存活检查；健康与指标端点仍可访问。

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
  controller_name: edgion.io/gateway-controller
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
  ha_mode: leader-only
  controller_name: edgion.io/gateway-controller
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
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/server.crt
      key: certs/grpc/server.key
      ca: certs/grpc/ca.crt
```

### 非 Kubernetes（虚拟机 / 裸金属）HA 部署

让两个（或更多）Controller 连接同一个 etcd 集群，实现主备 HA。Leader Election **始终开启**——恰好一个
Controller 成为 Leader，其余待命并在故障时接管。网关列出每个 Controller 的 `grpc_addr`，并轮换到当前的
Leader。

Controller **A**（`10.0.0.11`）：

```yaml
conf_center:
  type: etcd
  controller_name: edgion.io/gateway-controller
  endpoints: ["http://10.0.0.1:2379", "http://10.0.0.2:2379", "http://10.0.0.3:2379"]
  key_prefix: /edgion
  leader_election:
    identity: controller-a          # 可选；仅用于展示（日志 + 409 响应体）
    grpc_addr: 10.0.0.11:12151      # 可路由；网关轮换到的 conf_sync gRPC 端口
    admin_addr: 10.0.0.11:12101     # 可路由；非 Leader 409 响应体中指明的地址
    lease_duration_secs: 15
    renew_deadline_secs: 10
    retry_period_secs: 2
```

Controller **B**（`10.0.0.12`）除地址外完全相同：

```yaml
  leader_election:
    identity: controller-b
    grpc_addr: 10.0.0.12:12151
    admin_addr: 10.0.0.12:12101
```

每个网关列出两个 Controller 并轮换到当前 Leader（单元素列表即保持今天的单 Controller 行为）：

```yaml
# edgion-gateway.yaml
gateway:
  server_addrs:
    - "https://10.0.0.11:12151"
    - "https://10.0.0.12:12151"
```

**注意事项（详见上文 [conf_center.leader_election] — Etcd 模式）：**

- **必须提供 `v3election`。** Controller 在启动时探测它；若托管 / 代理的 etcd 禁用了它，则快速失败。
- **conf_sync mTLS 需手动签发证书**——`auto_ca` 仅限 Kubernetes。**服务端**证书的 SAN 必须覆盖**每一个**
  Controller 的 `grpc_addr`（此处即 `10.0.0.11` 和 `10.0.0.12`），否则网关在故障转移后会 TLS 校验失败：

  ```bash
  edgion-cli gen-certs --output file --output-dir /usr/local/edgion/certs/grpc \
      --server-san 10.0.0.11 --server-san 10.0.0.12
  ```

- **备节点的 `/ready` 返回 `503`。** 不要把 Admin `/ready` 当作普通存活检查，否则会重启健康的备节点。

## 相关文档

- [Gateway 配置](./gateway-config.md)
- [EdgionGatewayConfig CRD](./edgion-gateway-config.md)
- [工作目录行为](../../dev-guide/work-directory.md)
- [联邦 RBAC](../federation-rbac.md)
