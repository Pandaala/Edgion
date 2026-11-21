# Gateway 配置

edgion-gateway 通过 YAML 文件启动，默认路径为 `config/edgion-gateway.yaml`，可通过 `--config-file` 指定。

## 完整示例

```yaml
# 工作目录——所有相对路径均基于此目录
# 优先级：CLI --work-dir > 环境变量 EDGION_WORK_DIR > 此值 > "."
work_dir: "."

gateway:
  server_addrs: ["http://127.0.0.1:12151"]  # 必填：Controller gRPC 地址
  admin_listen: "127.0.0.1:12001"        # Admin API 监听地址（默认：127.0.0.1:12001，仅 loopback）
  # probe_listen: "0.0.0.0:12000"        # 存活/就绪探针端口（默认：0.0.0.0:12000）；绑定 0.0.0.0 以便 kubelet 可通过 Pod IP 访问
  # geoip:                               # 必须嵌套在 gateway: 下 —— 写在顶层的 geoip: 会被忽略，
  #                                       # 并产生日志级别为 ERROR 的启动诊断。见 docs/zh-CN/ops-guide/gateway/geoip.md
  #   db_path: /var/lib/edgion/geoip/db.mmdb
  #   update_url: https://mirror.internal/geo.mmdb

# 为便于平滑发布，进程 YAML 中的普通未知字段会被忽略，但 Gateway 会输出包含完整字段路径及
# action="ignored"、级别为 ERROR 的启动日志。已有的严格子结构（包括 geoip、path_normalization、
# request_mirror 和 access_log_options）仍会拒绝未知字段并中止启动。

# ─── gRPC 通道安全（必填）─────────────────────────────────────────
# 省略此段会导致 Gateway 拒绝启动。必须从下面三个选项中选择其一，
# 且需与 Controller 侧的配置保持一致。
#
# 选项 1 —— Kubernetes：由 Controller 内置 CA 自动签发客户端证书。
# 注意：在 GATEWAY 侧，只写 `auto_ca` 是不够的 —— auto_ca 意味着强制 mTLS，因此必须同时
# 配置证书路径，否则 Gateway 会以非零码退出（"auto_ca=true but no cert paths are configured"）。
# 这些路径指向 Controller 自动签发的 Secret（`edgion-conf-sync-certs`），官方 manifest 将其
# 挂载在 /etc/edgion/conf-sync-certs/。只有 CONTROLLER 侧可以只写 `auto_ca: true`
# （它在内存中自签服务端身份）。
conf_sync_security:
  auto_ca: true
  active: primary
  certs:
    - name: primary
      cert: /etc/edgion/conf-sync-certs/client.crt
      key: /etc/edgion/conf-sync-certs/client.key
      ca: /etc/edgion/conf-sync-certs/ca.crt
#
# 选项 2 —— 文件模式 mTLS：
# conf_sync_security:
#   active: primary
#   certs:
#     - name: primary
#       cert: certs/grpc/client.crt   # Gateway 客户端证书（PEM）
#       key: certs/grpc/client.key    # 客户端私钥（PEM）
#       ca: certs/grpc/ca.crt         # 验证 Controller 服务端证书的 CA
#
# 选项 3 —— 显式选择明文（仅限本地开发 / 非生产，会输出 ERROR 日志）：
# conf_sync_security:
#   active: insecure
#   certs:
#     - name: insecure
#       skip_tls: true

logging:
  # 系统日志（不影响业务日志路径）
  log_dir: logs
  log_prefix: edgion-gateway
  log_level: info               # 支持模块级：info,edgion::core::gateway::routes=debug
  json_format: false
  console: true
  buffer_size: 10000

server:
  threads: 0                               # 0 = 自动使用 CPU 核心数
  # downstream_tls_offload_threadpools: 2       # 可选，1..=4
  # downstream_tls_offload_thread_per_pool: 2  # 可选，1..=4；必须同时设置
  work_stealing: true                      # Tokio 任务窃取
  grace_period_seconds: 30                 # 优雅关闭等待秒数
  graceful_shutdown_timeout_seconds: 10    # 关闭超时秒数
  upstream_keepalive_pool_size: 128        # 上游 keepalive 连接池大小
  # error_log: logs/pingora_error.log      # Pingora 内部错误日志路径

# ─── 业务日志 ──────────────────────────────────────────────────────
# 每种日志独立配置路径，logging.log_dir 不影响这些路径

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # queueSize: 80000                   # 默认 cpu_cores × 10000
      # rotation:
      #   strategy: daily                  # daily | hourly | never | { Size: 104857600 }
      #   maxFiles: 7
      #   checkIntervalSecs: 30

ssl_log:
  enabled: true
  output:
    localFile:
      path: logs/ssl.log

tcp_log:
  enabled: false
  output:
    localFile:
      path: logs/tcp_access.log

tls_log:
  enabled: true
  output:
    localFile:
      path: logs/tls_access.log

udp_log:
  enabled: false
  output:
    localFile:
      path: logs/udp_access.log

# ─── 限流插件全局配置 ──────────────────────────────────────────────

rate_limit_local:
  default_estimator_slots_k: 64   # CMS 默认精度（× 1K slots）
  max_estimator_slots_k: 1024     # CMS 最大精度上限
  gateway_instance_count: 1       # 集群限流：实例数（无 Controller 推送时的静态 fallback）

# ─── OIDC introspection TinyUFO 限制 ─────────────────────────────

tiny_ufo_ttl_cache:
  default_max_entries: 1024
  default_max_logical_bytes: 4MiB

# LRU 默认值，包括 OIDC 刷新结果缓存。
lru_cache:
  default_capacity: 1024
  default_shards: 0

# ─── 日志脱敏（默认 replace）────────────────────────────────────

access_log_sensitive:
  mode: replace                    # off | replace | wrap
  # replace_with: "***"            # mode=replace 时的替换字符串
  # wrap_prefix: "<E>"             # mode=wrap 时的前缀
  # wrap_suffix: "</E>"            # mode=wrap 时的后缀
```

## 配置参数

### gateway

Gateway 采用**三端口模型**：

| 端口 | 默认绑定 | 用途 |
|------|---------|------|
| Admin | `127.0.0.1:12001` | Admin API（`/configclient`、`/api/v1/server-info` 等）——仅 loopback，不对外暴露 |
| Probe | `0.0.0.0:12000` | 存活/就绪探针（`/health`、`/ready`）——绑定所有接口，以便 kubelet 通过 Pod IP 探活 |
| Metrics | `0.0.0.0:12090` | Prometheus 指标——独立监听器，对外暴露 |

探针端口**不会**加入 gateway Service；kubelet 直接通过 Pod IP 探测 12000 端口。

> **运维变更提示（Breaking Change）**：不再支持通过 Pod IP 或 Kubernetes Service 远程访问 gateway admin API（`/configclient`、`/api/v1/server-info`）。请改用 `kubectl port-forward pod/<gw-pod> 12001:12001`（连接到 Pod 的 loopback）或 `kubectl exec`；**不要**通过 gateway Service 访问。`edgion-cli --target client` 默认连接 `localhost:12001`，不受影响。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `server_addrs` | `Array<String>` | **必填** | Controller conf_sync gRPC 地址列表，如 `["http://127.0.0.1:12151"]`。单地址原地重连；多地址按 Controller 的服务模式探测候选并切换，见 [Controller 故障切换](#controller-故障切换)。CLI 的 `--server-addr` 可重复传递 |
| `admin_listen` | `String` | `127.0.0.1:12001` | Admin API 监听地址，默认绑 loopback（`127.0.0.1:12001`）——敏感端点（`/configclient`、`/api/v1/server-info`）不对集群暴露。远程访问请使用 `kubectl port-forward pod/<name> 12001:12001`。 |
| `probe_listen` | `String` | `0.0.0.0:12000` | 存活/就绪探针监听地址，绑定所有接口以便 kubelet 通过 Pod IP 访问。仅提供 `/health` 和 `/ready`，不加入 gateway Service。 |
| `geoip` | `GeoIpConfig` | *(空——未设置 `db_path`)* | GeoIP 数据库与更新器配置，供 `GeoIpLocation` 插件使用（HTTP `EdgionPlugin` 和 L4 `EdgionStreamPlugin` 共用同一套配置结构）。**必须嵌套在 `gateway:` 下**——完整字段参考及必读的坑见 [GeoIP 地理限制](../gateway/geoip.md)。 |
| `outbound_egress_blocked_cidrs` | `Array<String>` | `[]`（随项目发布的配置文件列出了云元数据端点） | Gateway 后端和接入出站策略的客户端拒绝的额外目的 CIDR。只能追加；修改需重启；任一条目非法则启动失败。见 [出站封禁 CIDR](#出站封禁-cidr)。 |

### Controller 故障切换

配置多个 `server_addrs` 时，Gateway 会在启动时随机打乱列表，并保持一套当前配置同步会话。
当前会话异常后，Gateway 按需探测候选节点，只有候选成功响应 `GetServerInfo` 才切换，
并沿用已经验证的通道进行同步。能够连接但暂时不能提供配置的 Controller 不会被选中：
leader-only 部署要求节点是能够提供服务的 leader，all-serve 部署可使用任意就绪副本。
服务资格由 Controller 执行检查；探测成功不代表获得领导权租约，也不保证后续流一定成功。

启动时会检查全部地址的 gRPC URI 语法。即使其他地址可用，无效地址也会导致启动失败；
修正地址列表后再启动。此项检查不要求远端节点在线。

不可用的候选不会替换当前已安装通道。恢复过程遍历地址列表，也会重新尝试原地址，
在完整一圈远端尝试失败后退避。本地证书或通道准备失败会保留候选，等待后重试。
恢复期间 Gateway 继续使用已有配置；新启动的 Gateway 仍需完成初始配置同步后才能服务。
只有一个地址时，Gateway 重连该地址，不执行端点轮转。

证书重载使用本地验证通过的新材料重建当前已安装地址，即使 Controller 暂时不可达也可完成。
并发探测若属于旧连接代次，其结果不能覆盖此次重载。无需新增健康检查 RPC、常驻备用连接，
也无需增加配置项。

### System CA 包（启动前提）

`wellKnownCACertificates: System`（以及未配置 CA）使用固定路径的 OS CA 包：按顺序取 `/etc/ssl/certs/ca-certificates.crt`、`/etc/pki/tls/certs/ca-bundle.crt`、`/etc/ssl/cert.pem` 中第一个存在的文件。
它没有配置项。后端 TLS 与 Gateway 所有出向客户端共用这个文件。找不到这样的文件，或者它不是普通文件、
超过 8 MiB、PEM 格式错误、没有可用证书、BoringSSL 无法加载时，Gateway 拒绝启动。官方镜像和安装包都会安装
`ca-certificates`，自定义镜像也必须安装。要信任私有 CA，请使用 `caCertificateRefs`（BackendTLSPolicy、
资源自身或 EdgionGatewayConfig `outboundTls`），它会替换 `System`。参见[后端 TLS](../../user-guide/http-route/backends/backend-tls.md)。

### 出站封禁 CIDR

`gateway.outbound_egress_blocked_cidrs` 在 Gateway 内置拦截之外，为后端和接入出站策略的客户端追加拒绝连接的目的地址段。内置拦截包括回环 `127.0.0.0/8` / `::1`（沿用现有的回环例外）和链路本地
`169.254.0.0/16` / `fe80::/10`，后者已覆盖常见的云元数据地址 `169.254.169.254`。本字段针对的是
位于链路本地**之外**的云元数据与平台端点：这些地址在别的网络里可能是正常业务地址，内置分类器
无法替你判断。

```yaml
gateway:
  outbound_egress_blocked_cidrs:
    - "100.100.100.200/32"     # 阿里云 ECS 实例元数据
    - "fd00:ec2::254/128"      # AWS EC2 实例元数据（IPv6）
    - "fd20:ce::254/128"       # Google Cloud 元数据服务器（IPv6）
    - "fd00:c1::a9fe:a9fe/128" # Oracle Cloud 实例元数据（IPv6）
    - "168.63.129.16/32"       # Azure 平台端点（WireServer）
```

- **默认值**：代码默认值（不写该键）为空列表。随项目发布的配置文件（`config/edgion-gateway.yaml`、
  Kubernetes ConfigMap 以及 standalone 安装包）列出了上面的端点。只有当某个地址在你的网络中是
  合法目的地时才删除对应条目；元数据服务使用 `169.254.169.254` 或其他链路本地地址的厂商，
  已被内置拦截覆盖。
- **只能追加**：该列表只能增加拦截，不能放开任何内置地址段。即使条目覆盖了回环或链路本地地址，
  这两类地址仍按内置规则处理。列表中的地址无论 `outbound_egress_block_private`、
  `target.blockPrivate`、`allowLoopbackUpstream` 如何设置都会被拒绝。
- **生效范围**：HTTP/gRPC 和 L4 后端、RequestMirror、DirectEndpoint、AllEndpointStatus、主动健康探针，
  以及接入出站策略的客户端，包括 webhook、ForwardAuth、OIDC、AI 提供方、DSL HTTP 调用、Wasm 外呼和下载、
  GeoIP 更新及 HTTP DNS。Gateway 在负载均衡前过滤 DNS 派生的 Service 端点，包括 Controller 发布的 DNS 切片。
  Controller 不读取该配置。Kafka broker 使用 SDK 拨号，Elasticsearch LinkSys 使用独立的客户端配置；
  这两个连接器的目的地址限制需要通过网络层实施。
- **格式**：每个条目必须使用 `地址/前缀长度` 形式（只写地址会被拒绝）；IPv4-mapped 的 IPv6 目的地址
  会命中对应的 IPv4 条目。任一条目非法则启动失败。修改需重启 Gateway；没有对应的环境变量或 CLI 参数。
- **可观测性**：HTTP 后端地址检查可在访问日志中记录 `BackendAddrBlockedCidrNotAllowed`。
  L4 路由沿用原有的不安全上游状态。选址前过滤的 DNS 答案没有对应的请求诊断。
  接入指标的出站解析器每过滤一个命中地址，就增加一次 `edgion_outbound_egress_blocked_total{reason="blocked_cidr"}`。
  托管出站 URL 的 IP 字面量被后端地址检查拒绝时，该计数器不会递增。

这是纵深防御。针对这些地址的 Kubernetes `NetworkPolicy`（或主机防火墙）出站拒绝规则仍是主要控制手段。

### conf_sync_security

**必填配置。** conf_sync gRPC 通道会把解析后的 Secret（TLS 私钥、后端凭证）下发到 Gateway，
因此其传输方式必须由运维显式选择。

**完全省略 `conf_sync_security` 段会导致 Gateway 拒绝启动**（非零退出码，并打印可操作的错误信息）。
必须且只能配置以下之一：

| 配置状态 | 结果 |
|---------|------|
| `auto_ca: true` **并且**配置选中的 `certs` 证书路径 | mTLS —— 由 Controller 内置 CA 自动签发客户端证书（Kubernetes 推荐） |
| **只写** `auto_ca: true`、没有证书路径 | **拒绝启动** —— auto_ca 意味着强制 mTLS，Gateway 必须知道自动签发的证书文件在哪里 |
| 选中的 `certs` 条目，含 `cert` + `key` + `ca` | 文件模式 mTLS |
| `skip_tls: true` | 显式选择明文（输出 ERROR 日志；仅限本地开发 / 非生产） |
| *（省略该段）* | **拒绝启动** |

> **`auto_ca` 两端不对称——不要直接照抄 Controller 的配置块。** **Controller** 只需要
> `auto_ca: true`：它在内存中自签服务端身份。**Gateway** 则需要 `auto_ca: true` **加上**
> 选中的 `certs` 证书路径，因为它的客户端证书是由 Controller 签发进 Secret、必须从磁盘读取的。
> Gateway 上只写 `auto_ca: true` 会以非零码退出，报
> `conf_sync_security.auto_ca=true but no cert paths are configured`。

该选择必须与 Controller 侧一致——两端不匹配会导致握手失败。

> **迁移提示**：过去省略该段意味着"明文 gRPC"，依赖此行为的部署现在将无法启动。请从以下三者中选择其一：
>
> ```yaml
> # (a) Kubernetes —— auto_ca + 自动签发的证书路径（与官方 ConfigMap 一致）
> conf_sync_security:
>   auto_ca: true
>   active: primary
>   certs:
>     - name: primary
>       cert: /etc/edgion/conf-sync-certs/client.crt
>       key: /etc/edgion/conf-sync-certs/client.key
>       ca: /etc/edgion/conf-sync-certs/ca.crt
>
> # (b) 文件模式 mTLS
> conf_sync_security:
>   active: primary
>   certs:
>     - { name: primary, cert: certs/grpc/client.crt, key: certs/grpc/client.key, ca: certs/grpc/ca.crt }
>
> # (c) 继续使用明文，显式声明（不安全）
> conf_sync_security:
>   active: insecure
>   certs:
>     - { name: insecure, skip_tls: true }
> ```

#### conf_sync_security.certs

`certs` 是证书集列表，`active` 按 `name` 选择条目；仅有一个条目时可以省略。
重复名称和未匹配的选择器会被拒绝。旧的单套 `tls` 配置必须迁移为 `certs` 条目。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | `String?` | — | 条目名称，供 `active` 选择 |
| `cert` | `String` | — | 客户端证书路径（PEM，相对 work_dir） |
| `key` | `String` | — | 客户端私钥路径（PEM） |
| `ca` | `String` | — | CA 证书路径，用于验证 Controller 服务端证书 |
| `skip_tls` | `Boolean` | `false` | 显式明文开关，`true` 时跳过 TLS 以明文连接，会输出 ERROR 日志并置位 `edgion_system_error_state{reason="skip_tls"}` |

启用 mTLS 时，`gateway.server_addrs` 的 URI scheme 会自动切换为 `https://`。证书可通过 `edgion-cli gen-certs --output file --output-dir <dir>` 生成。

### logging

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `log_dir` | `String` | `logs` | 系统日志目录，**不影响业务日志路径** |
| `log_prefix` | `String` | `edgion-gateway` | 日志文件名前缀 |
| `log_level` | `String` | `info` | 日志级别：`trace`、`debug`、`info`、`warn`、`error`，支持模块级过滤 |
| `json_format` | `Boolean` | `false` | 是否以 JSON 结构化格式输出日志 |
| `console` | `Boolean` | `true` | 是否同时输出到控制台 |
| `buffer_size` | `Integer` | `10000` | 异步日志缓冲区大小 |

### server

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `threads` | `Integer` | CPU 核心数 | Pingora 工作线程数，`0` 表示自动检测 |
| `downstream_h2_stream_window_size` | `Integer` | `65535` | 每条 HTTP/2 流的接收窗口，单位字节；范围 `65535`–`16777216` |
| `downstream_h2_connection_window_size` | `Integer` | `65535` | 每个 HTTP/2 连接的接收窗口，单位字节；范围 `65535`–`16777216` |
| `downstream_h2_max_concurrent_streams` | `Integer` | `100` | 每个 HTTP/2 连接允许的客户端并发流数；范围 `1`–`1024` |
| `max_downstream_connections` | `Integer` | 不限制 | 进程级下游并发连接上限，覆盖所有 HTTP/HTTPS/TCP/TLS 监听器（不含 UDP）；`≥ 1`。超限连接在 accept 时、PROXY 解析和 TLS 握手之前关闭 |
| `downstream_tls_offload_threadpools` | `Integer` | 关闭 | 每个终止 TLS 的监听器使用的专用握手线程池数，范围 `1`–`4`；必须同时设置另一字段 |
| `downstream_tls_offload_thread_per_pool` | `Integer` | 关闭 | 每个握手线程池的线程数，范围 `1`–`4`；必须同时设置另一字段 |
| `work_stealing` | `Boolean` | `true` | 启用 Tokio 任务窃取，提高 CPU 利用率 |
| `grace_period_seconds` | `Integer` | `30` | 优雅关闭期间等待连接结束的秒数 |
| `graceful_shutdown_timeout_seconds` | `Integer` | `10` | 优雅关闭超时，超过后强制终止 |
| `upstream_keepalive_pool_size` | `Integer` | `128` | 上游 keepalive 连接池大小 |
| `error_log` | `String` | — | Pingora 内部错误日志文件路径 |

> 这些设置仅属于本地 Gateway 进程，在启动时解析，修改后需要重启。GatewayClass 级别的
> HTTP 压缩和下游连接请求数上限由 `EdgionGatewayConfig.spec.server` 配置。

下游 HTTP/2 参数统一应用于支持 h2c 的 HTTP 监听器和协商 H2 的 HTTPS 监听器。
省略时保留 Pingora 默认值，包括 64 KiB 解码后请求头限制。例如，允许单流接收窗口
为 256 KiB、连接接收窗口为 1 MiB，每连接最多 100 条并发流：

```yaml
server:
  downstream_h2_stream_window_size: 262144
  downstream_h2_connection_window_size: 1048576
  downstream_h2_max_concurrent_streams: 100
```

生效的单流窗口乘以并发流上限不得超过 `67108864` 字节（64 MiB）。这是每连接的
保守配置额度预算，不代表进程内存硬上限，也不限制整个请求体大小。无效值或组合
会在启动服务前被拒绝。三个字段均可单独设置，省略的字段保留各自默认值。
CLI 参数 `--downstream-h2-stream-window-size`、`--downstream-h2-connection-window-size`
和 `--downstream-h2-max-concurrent-streams` 分别覆盖对应 YAML 字段。窗口必须填写整数
字节数，不能使用容量字符串。修改后需要重启 Gateway，参数用于重启后建立的连接，
不会热更新已有连接。这些字段不会开启被禁用的 HTTP/2，不改变上游连接窗口，也不
改变 HTTP/1 行为。高延迟链路上的上传或客户端发送的 gRPC 消息可能受益于较大的
接收窗口，但应先测量实际资源消耗和吞吐；它们不会直接扩大客户端的下载接收窗口。

`max_downstream_connections` 约束整个进程，不依赖任何 Gateway 或路由配置，它限制的是下游 TCP 类 socket，也就是与上游连接、配置同步和日志共享的文件描述符中最大的消耗者；一个 HTTP/2 socket 仍可能带出最多 `downstream_h2_max_concurrent_streams` 个上游连接。UDP 监听器不在覆盖范围内（每个新的 UDP 来源还会绑定一个上游 socket）。取值应低于进程 `NOFILE` 上限，并为上游 socket 留出余量（常见起点约为其一半）。

socket 从 accept 开始计数，直到关闭为止，包括仍处于监听器准入中（例如正在等待依赖 Redis 的 stream plugin）、TLS 握手中（最长 60 秒）以及空闲的 keep-alive 或 TCP 转发期间。`edgion_downstream_connections_active` 统计的是已完成监听器准入的 socket，因此在 `edgion_downstream_connections_rejected_total{reason="global"}` 增长时，它可能略低于上限。超限连接不返回响应直接关闭。

该上限由所有监听器共享：一个客户端在某个监听器上占住大量空闲 socket，就可能让其他所有监听器都达到上限。请配合低于进程上限的单监听器 [`ConnectionLimit`](../../user-guide/tcp-route/stream-plugins.md#connectionlimit-并发连接上限) 来隔离监听器，并设置握手与空闲超时。Admin、探活和 metrics 端口不计入，达到上限时仍可访问。CLI 参数 `--max-downstream-connections` 覆盖 YAML 字段；`0` 会被拒绝。

该上限作用于每个 Gateway 进程（每个 pod），而不是整个集群。滚动重启期间，新旧 pod 各自最多接纳到上限。

TLS 握手卸载仅用于 HTTPS 和终止 TLS 的监听器。每个实际监听器拥有独立线程池，
`监听器数 × 线程池数 × 每池线程数` 不得超过 64；无效或只设置一个字段会使启动失败。
线程在首次握手时创建，因此启动时的配置校验不能保证之后创建线程一定成功。
卸载本身不限制并发握手，也不为已有流量保留 CPU；需要时应另外配置连接准入。
启用前，应在相同 CPU 限额下比较开关前后的实际流量表现；小型部署或大量复用连接的
部署可能因新增线程而变慢。若需回退，删除这两个字段并重启 Gateway。
对应的 CLI 参数是 `--downstream-tls-offload-threadpools` 和
`--downstream-tls-offload-thread-per-pool`；显式传入的参数覆盖对应 YAML 字段。

### 业务日志

所有业务日志（`access_log`、`ssl_log`、`tcp_log`、`tls_log`、`udp_log`）共享相同 Schema：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `Boolean` | — | 是否启用此类日志 |
| `output.localFile.path` | `String` | — | 日志文件路径（相对 work_dir），与 `logging.log_dir` 无关 |
| `output.localFile.queueSize` | `Integer` | CPU 核心数 × 10000 | 写入队列大小 |
| `output.localFile.rotation.strategy` | `Enum` | — | 轮转策略（见下表） |
| `output.localFile.rotation.maxFiles` | `Integer` | — | 保留的轮转文件数，`0` 表示无限 |
| `output.localFile.rotation.checkIntervalSecs` | `Integer` | `30` | 时间型策略（`daily` / `hourly`）的检查间隔秒数。`Size` 策略在每批写入后检查，不受此字段影响。 |

**`rotation.strategy` 可选值：**

| 值 | 说明 |
|----|------|
| `"daily"` | 每日零点轮转 |
| `"hourly"` | 每小时轮转 |
| `"never"` | 不轮转 |
| `{ Size: 104857600 }` | 按文件大小轮转（字节），默认 100 MB |

### request_mirror

在 Gateway 进程 YAML 中配置顶层区块：

```yaml
request_mirror:
  max_concurrent: 1024
  max_buffered_bytes: 64MiB
```

`max_concurrent` 限制进程内镜像任务总数，范围为 `1`–`65536`。
`max_buffered_bytes` 限制镜像持有的请求体数据，范围为 `1` 字节–`1GiB`。
上述为默认值，两者均可配置，修改后需要重启 Gateway。所有路由和插件实例共享预算，
包括热加载期间共存的新旧插件实例。预算耗尽时跳过或取消镜像副本，主请求继续处理。
非法值或未知字段会导致启动失败；零值不表示无限制。这两个字段没有 CLI 或环境变量覆盖入口。

字节预算不是进程 RSS 上限，请求头、TLS 和连接开销另行存在。单个镜像的限制仍通过
[HTTPRoute 镜像注解](../../user-guide/http-route/filters/gateway-api/request-mirror.md)
或 EdgionPlugins 内联配置设置，标准 Gateway API 字段保持不变。

### rate_limit

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `default_estimator_slots_k` | `Integer` | `64` | CMS 默认 slot 数（× 1K），内存约为 `slots_k × 64 KB` |
| `max_estimator_slots_k` | `Integer` | `1024` | CMS 最大 slot 数上限 |
| `gateway_instance_count` | `Integer` | `1` | Gateway 实例数，集群限流时作为分母，**集群部署时必须设置正确值** |

### access_log_sensitive

TLS 代理日志（全局及各 listener）会脱敏后端 `upstream_info.addr`，以及连接错误和重试信息中的目的地址片段。独立的后端端口及周围的错误诊断保持可读。TLS 代理和 SSL 握手日志中的 `sni` 在所有模式下均保持明文；下文所述 HTTP access log 的 SNI 脱敏行为保持不变。其他 TLS/SSL 错误文本不会被整段脱敏。`wrap` 仅标记地址片段，仍保留原始值。

控制 access log、ssl.log、plugin log 中已标注敏感字段的脱敏行为。**默认 `mode=replace`**（已从旧默认 `off` 切换）——敏感字段开箱即渲染为 `***`。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `mode` | `Enum` | `"replace"` | 脱敏模式：`"off"`（关闭，明文）/ `"replace"`（整体替换）/ `"wrap"`（包裹标记） |
| `replace_with` | `String` | `"***"` | `mode=replace` 时整体替换敏感值的字符串 |
| `wrap_prefix` | `String` | `"<E>"` | `mode=wrap` 时附加在敏感值前的前缀 |
| `wrap_suffix` | `String` | `"</E>"` | `mode=wrap` 时附加在敏感值后的后缀 |

**三种模式的行为差异**（以 `direct_peer_ip=1.2.3.4` 为例）：

| 模式 | 输出 | 用途 |
|------|------|------|
| `off` | `"1.2.3.4"` | 明文，便于运维和调试 |
| `replace` | `"***"` | 默认，完全屏蔽敏感值，最强保护 |
| `wrap` | `"<E>1.2.3.4</E>"` | 保留原值，但标记敏感片段，便于下游日志系统识别后做二次脱敏或审计 |

**生效范围**——比内置字段列表本身更大，不要假设它只覆盖内置字段：

- **access log**：`direct_peer_ip`、`remote_ip`、`path`、`x_forwarded_for`、`sni`、`request_uri`、客户端证书的 `subject`/`sans`/`cn`
- **`AccessLogExtern` 声明的 `ext` 字段**（`EdgionPlugins.spec.accessLogExtern`）遵循同一个开关：`routeLabel`/`routeAnnotation` 来源永远明文，其余来源除非在 `EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys` 中列入白名单，否则按此 `mode` 渲染（白名单未命中仍会输出该字段，只是被脱敏，而非丢弃）。详见 [Access Log](../observability/access-log.md#扩展日志accesslogextern)。
- **plugin log**：13 个插件中 46 处通过 `sensitive(...)` 标注的参数（real_ip 的 IP、basic/key/ldap/jwt/hmac 的用户名、direct_endpoint 的地址、region_route 的 key/region/host、proxy_rewrite 的 method 等）；DSL 脚本里 `log_warn(value)` / `log_error(value)` 的用户参数
- **ssl.log**：mTLS 白名单校验失败时错误信息里的客户端证书字段

**默认切换为 `replace` 的原因。** 由于 `ext` 渲染复用同一个开关，`mode=replace` 默认值是让 `AccessLogExtern` "默认脱敏" 成立的唯一因素。**不存在既保留可读 `path`、又对 `ext` 中的 cookie 脱敏的配置**：`off` 会让白名单失效（一切都明文，包括 `ext`）；`wrap` 会把原始值写进网关本地日志文件/stdout（用 `<E>...</E>` 标记以便下游 ES/ClickHouse 管道剥离，但网关自己的本地文件已经是明文）。这是刻意的单开关设计，不是疏漏——需要可读 path 时请在这两种权衡里二选一。同时注意 `access_log_options.includeRawUri` 的交互：`replace` 默认下 `request_uri` 本身也会渲染为 `***`，仅开启该开关不再能得到可读的原始视图。

**注意**：Wrap 模式自定义前后缀时，建议总长度（`wrap_prefix.len() + wrap_suffix.len()`）不超过 36 字节，否则会压缩 plugin log 单条目可写长度。

**仓库自带的 `config/edgion-gateway.yaml`（本文档开头所说的默认路径文件）本身就显式写了 `access_log_sensitive.mode: off`，而不是走上面说的 `replace` 默认值。** 这个文件是本地开发 / 集成测试 harness 专用配置——集成测试断言 `path` 以及经 `sensitive(...)` 标注写入的 plugin log 值，需要读到未脱敏的真实值。只有当配置文件里完全不写 `access_log_sensitive` 段时，才会落到 `replace` 默认值；直接拿仓库自带的这份文件启动进程，走的是文件里显式写死的 `off`，不是默认值。发布镜像不打包任何配置文件（`cicd/docker/Dockerfile.runtime`），因此 Kubernetes 部署不受这份仓库文件影响，会拿到 `replace` 默认值（除非自己的部署配置也显式写了 `access_log_sensitive` 段）。

## 常见配置场景

### 连接远端 Controller

```yaml
gateway:
  server_addrs: ["http://10.0.0.5:12151"]
```

### 开启业务日志按日轮转

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      rotation:
        strategy: daily
        maxFiles: 30
        checkIntervalSecs: 30
```

### 集群限流配置

当 Gateway 以多副本运行时，必须设置 `gateway_instance_count` 以确保限流计数准确：

```yaml
rate_limit_local:
  default_estimator_slots_k: 64
  max_estimator_slots_k: 1024
  gateway_instance_count: 3   # 部署了 3 个 Gateway 副本
```

### OIDC Introspection TinyUFO 限制

```yaml
tiny_ufo_ttl_cache:
  default_max_entries: 1024
  default_max_logical_bytes: 4MiB
```

OIDC introspection 未配置 `tinyUfoTtlCache` 覆盖值时使用这些进程默认值。
覆盖配置必须同时设置 `maxEntries` 和 `maxLogicalBytes`。总逻辑预算控制单个
TinyUFO 实例的记录权重，不是精确的分配器或 RSS 上限；Gateway 进程的硬上限应由
Kubernetes/cgroup 内存限制保证。OIDC 短期刷新结果缓存与数据面上游 DNS 应答缓存
（AI 供应商端点、外部跳转以及 `system` 模式 ExternalName Service 的新 UDP 会话）使用
独立的 `lru_cache` 默认值；
该默认值也是 DNS 缓存在主机名、解析器变动下的条目数上界。DNS 缓存只沿用容量，
分片数另有下限——每次 AI 尝试和外部跳转请求都会读取它，因此把
`default_shards` 设成低于该下限并不会降低它的分片数。LDAP 认证不缓存。绝对安全上限分别为 65,536 个
缓存条目和 4,194,304 个 TinyUFO 权重单位（按 64 字节逻辑单位计为 256 MiB）；
这些上限不是默认值。

### 启用日志脱敏

`mode: replace` 现在已是默认值，内置敏感字段（用户 IP、路径、`AccessLogExtern` 未列入白名单的 `ext` 字段等）开箱即被屏蔽。仅在需要自定义 `replace_with`、或需要主动切回明文时才显式写这个段：

```yaml
access_log_sensitive:
  mode: replace
  replace_with: "***"
```

如需保留原值同时为下游审计系统打标，使用 `wrap` 模式：

```yaml
access_log_sensitive:
  mode: wrap
  wrap_prefix: "<sens>"
  wrap_suffix: "</sens>"
```

输出示例：`"direct_peer_ip": "<sens>1.2.3.4</sens>"`，下游可通过正则识别 `<sens>...</sens>` 区间做二次处理。

### 上游 RemoteIp Header

`EdgionGatewayConfig.spec.forwardedHeaders.remoteIpHeader` 控制 Gateway 在
HTTP 上游请求中传递最终 `RemoteIp` 的 Header 名称。默认值为 `X-Real-IP`，与
现有行为及 Nginx 的常用约定一致，也可以配置为 `X-Edgion-Remote-IP` 等自定义
名称。名称会经过校验，不能使用 hop-by-hop Header、`Host`、`Content-Length`、
`X-Forwarded-For` 或其他保留传输 Header。所有 request 插件执行完毕后，Gateway
会用规范化的最终值覆盖同名入站 Header。

该输出配置与 `spec.realIp.realIpHeader` 相互独立：前者负责向上游传播已解析的
客户端身份，后者负责从可信代理提供的下游 Header 中建立 `RemoteIp`。

### 启用 mTLS 连接 Controller

```yaml
gateway:
  server_addrs: ["http://10.0.0.5:12151"]  # scheme 会自动切换为 https://

conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/client.crt
      key: certs/grpc/client.key
      ca: certs/grpc/ca.crt
```

## 相关文档

- [Controller 配置](./controller-config.md)
- [EdgionGatewayConfig CRD](./edgion-gateway-config.md)
- [GeoIP 地理限制](../gateway/geoip.md)
- [Access Log 格式](../observability/access-log.md)
- [工作目录行为](../../dev-guide/work-directory.md)

## 进程级请求体存储预算

启动配置 `gateway.request_body_budget` 由同一进程的全部监听器和 GatewayClass
共享，与支持热更新的 `EdgionGatewayConfig.spec.requestBody` 单请求策略独立。

```yaml
gateway:
  request_body_budget:
    memory_limit: 256MiB
    disk_limit: 2GiB
    max_captures: 1024
    max_files: 1024
```

所有值必须为正数；无效大小和未知字段会阻止启动。修改后需要重启。
内存预算统计存储拥有的正文分配容量，包括物化副本及仍被重放或快照保留的字节；
磁盘预算统计临时文件预留字节，`max_files` 限制打开的临时文件数量。
`max_captures` 在捕获对象、读取器及导出的正文引用全部释放后才归还额度。
这些默认值是安全上限，不代表进程总堆内存、RSS 或可用磁盘空间保证；传输缓冲、
插件解析和扫描结果、元数据、分配器开销以及系统页缓存不在字节预算内。
即使只有一次捕获，保留多个历史快照仍可能增加元数据。

无法预留容量的请求立即返回 HTTP 503，并记录访问日志诊断
`BodyCaptureCapacityExceeded`，不会加入容量等待队列。单请求正文超限仍返回 413；
现有存储 I/O 并发与超时限制继续生效。已经分离执行的尽力而为请求镜像在额度不足时
仅停止镜像，不改变主请求响应。
