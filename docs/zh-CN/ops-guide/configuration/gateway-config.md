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
  # geoip:                               # 必须嵌套在 gateway: 下 —— 写在顶层的 geoip: 会被静默丢弃，
  #                                       # 且没有任何报错。见 docs/zh-CN/ops-guide/gateway/geoip.md
  #   db_path: /var/lib/edgion/geoip/db.mmdb
  #   update_url: https://mirror.internal/geo.mmdb

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
  tls:
    cert: /etc/edgion/conf-sync-certs/client.crt
    key: /etc/edgion/conf-sync-certs/client.key
    ca: /etc/edgion/conf-sync-certs/ca.crt
#
# 选项 2 —— 文件模式 mTLS：
# conf_sync_security:
#   tls:
#     cert: certs/grpc/client.crt   # Gateway 客户端证书（PEM）
#     key: certs/grpc/client.key    # 客户端私钥（PEM）
#     ca: certs/grpc/ca.crt         # 验证 Controller 服务端证书的 CA
#
# 选项 3 —— 显式选择明文（仅限本地开发 / 非生产，会输出 ERROR 日志）：
# conf_sync_security:
#   tls:
#     skip_tls: true

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
  work_stealing: true                      # Tokio 任务窃取
  grace_period_seconds: 30                 # 优雅关闭等待秒数
  graceful_shutdown_timeout_seconds: 10    # 关闭超时秒数
  upstream_keepalive_pool_size: 128        # 上游 keepalive 连接池大小
  downstream_keepalive_request_limit: 1000 # 下游每连接最大请求数（0 = 无限）
  # error_log: logs/pingora_error.log      # Pingora 内部错误日志路径

# ─── 业务日志 ──────────────────────────────────────────────────────
# 每种日志独立配置路径，logging.log_dir 不影响这些路径

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # queue_size: 80000                  # 默认 cpu_cores × 10000
      # rotation:
      #   strategy: daily                  # daily | hourly | never | { Size: 104857600 }
      #   max_files: 7
      #   check_interval_secs: 30

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

rate_limit:
  default_estimator_slots_k: 64   # CMS 默认精度（× 1K slots）
  max_estimator_slots_k: 1024     # CMS 最大精度上限
  gateway_instance_count: 1       # 集群限流：实例数（无 Controller 推送时的静态 fallback）

# ─── OIDC introspection TinyUFO 限制 ─────────────────────────────

tiny_ufo_ttl_cache:
  default_max_entries: 1024
  default_max_logical_bytes: 4MiB

# LRU 默认值，包括 OIDC Access Token 缓存。
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
| `server_addrs` | `Array<String>` | **必填** | Controller conf_sync gRPC 地址列表，如 `["http://127.0.0.1:12151"]`。单地址与旧版单地址行为完全一致；多地址启用 etcd 模式 HA，Gateway 会在列表中轮转寻找 leader 并在故障时切换。CLI 的 `--server-addr` 可重复传递 |
| `admin_listen` | `String` | `127.0.0.1:12001` | Admin API 监听地址，默认绑 loopback（`127.0.0.1:12001`）——敏感端点（`/configclient`、`/api/v1/server-info`）不对集群暴露。远程访问请使用 `kubectl port-forward pod/<name> 12001:12001`。 |
| `probe_listen` | `String` | `0.0.0.0:12000` | 存活/就绪探针监听地址，绑定所有接口以便 kubelet 通过 Pod IP 访问。仅提供 `/health` 和 `/ready`，不加入 gateway Service。 |
| `geoip` | `GeoIpConfig` | *(空——未设置 `db_path`)* | GeoIP 数据库与更新器配置，供 `GeoIpLocation` 插件使用（HTTP `EdgionPlugin` 和 L4 `EdgionStreamPlugin` 共用同一套配置结构）。**必须嵌套在 `gateway:` 下**——完整字段参考及必读的坑见 [GeoIP 地理限制](../gateway/geoip.md)。 |

### conf_sync_security

**必填配置。** conf_sync gRPC 通道会把解析后的 Secret（TLS 私钥、后端凭证）下发到 Gateway，
因此其传输方式必须由运维显式选择。

**完全省略 `conf_sync_security` 段会导致 Gateway 拒绝启动**（非零退出码，并打印可操作的错误信息）。
必须且只能配置以下之一：

| 配置状态 | 结果 |
|---------|------|
| `auto_ca: true` **并且**配置 `tls` 证书路径 | mTLS —— 由 Controller 内置 CA 自动签发客户端证书（Kubernetes 推荐） |
| **只写** `auto_ca: true`、没有证书路径 | **拒绝启动** —— auto_ca 意味着强制 mTLS，Gateway 必须知道自动签发的证书文件在哪里 |
| `certs` / 旧格式 `tls` 段，含 `cert` + `key` + `ca` | 文件模式 mTLS |
| `tls.skip_tls: true` | 显式选择明文（输出 ERROR 日志；仅限本地开发 / 非生产） |
| *（省略该段）* | **拒绝启动** |

> **`auto_ca` 两端不对称——不要直接照抄 Controller 的配置块。** **Controller** 只需要
> `auto_ca: true`：它在内存中自签服务端身份。**Gateway** 则需要 `auto_ca: true` **加上**
> `tls` 证书路径，因为它的客户端证书是由 Controller 签发进 Secret、必须从磁盘读取的。
> Gateway 上只写 `auto_ca: true` 会以非零码退出，报
> `conf_sync_security.auto_ca=true but no cert paths are configured`。

该选择必须与 Controller 侧一致——两端不匹配会导致握手失败。

> **迁移提示**：过去省略该段意味着"明文 gRPC"，依赖此行为的部署现在将无法启动。请从以下三者中选择其一：
>
> ```yaml
> # (a) Kubernetes —— auto_ca + 自动签发的证书路径（与官方 ConfigMap 一致）
> conf_sync_security:
>   auto_ca: true
>   tls:
>     cert: /etc/edgion/conf-sync-certs/client.crt
>     key: /etc/edgion/conf-sync-certs/client.key
>     ca: /etc/edgion/conf-sync-certs/ca.crt
>
> # (b) 文件模式 mTLS
> conf_sync_security:
>   tls: { cert: certs/grpc/client.crt, key: certs/grpc/client.key, ca: certs/grpc/ca.crt }
>
> # (c) 继续使用明文，显式声明（不安全）
> conf_sync_security:
>   tls: { skip_tls: true }
> ```

#### conf_sync_security.tls

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
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
| `work_stealing` | `Boolean` | `true` | 启用 Tokio 任务窃取，提高 CPU 利用率 |
| `grace_period_seconds` | `Integer` | `30` | 优雅关闭期间等待连接结束的秒数 |
| `graceful_shutdown_timeout_seconds` | `Integer` | `10` | 优雅关闭超时，超过后强制终止 |
| `upstream_keepalive_pool_size` | `Integer` | `128` | 上游 keepalive 连接池大小 |
| `downstream_keepalive_request_limit` | `Integer` | `1000` | 下游每连接最大请求数，`0` 表示无限 |
| `error_log` | `String` | — | Pingora 内部错误日志文件路径 |

> 这些是进程级行为。如果需要调整 GatewayClass 级别的 server 行为，请使用 [EdgionGatewayConfig](./edgion-gateway-config.md)。

### 业务日志

所有业务日志（`access_log`、`ssl_log`、`tcp_log`、`tls_log`、`udp_log`）共享相同 Schema：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `Boolean` | — | 是否启用此类日志 |
| `output.localFile.path` | `String` | — | 日志文件路径（相对 work_dir），与 `logging.log_dir` 无关 |
| `output.localFile.queue_size` | `Integer` | CPU 核心数 × 10000 | 写入队列大小 |
| `output.localFile.rotation.strategy` | `Enum` | — | 轮转策略（见下表） |
| `output.localFile.rotation.max_files` | `Integer` | — | 保留的轮转文件数，`0` 表示无限 |
| `output.localFile.rotation.check_interval_secs` | `Integer` | `30` | 时间型策略（`daily` / `hourly`）的检查间隔秒数。`Size` 策略在每批写入后检查，不受此字段影响。 |

**`rotation.strategy` 可选值：**

| 值 | 说明 |
|----|------|
| `"daily"` | 每日零点轮转 |
| `"hourly"` | 每小时轮转 |
| `"never"` | 不轮转 |
| `{ Size: 104857600 }` | 按文件大小轮转（字节），默认 100 MB |

### rate_limit

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `default_estimator_slots_k` | `Integer` | `64` | CMS 默认 slot 数（× 1K），内存约为 `slots_k × 64 KB` |
| `max_estimator_slots_k` | `Integer` | `1024` | CMS 最大 slot 数上限 |
| `gateway_instance_count` | `Integer` | `1` | Gateway 实例数，集群限流时作为分母，**集群部署时必须设置正确值** |

### access_log_sensitive

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

**默认切换为 `replace` 的原因。** 由于 `ext` 渲染复用同一个开关，`mode=replace` 默认值是让 `AccessLogExtern` "默认脱敏" 成立的唯一因素。**不存在既保留可读 `path`、又对 `ext` 中的 cookie 脱敏的配置**：`off` 会让白名单失效（一切都明文，包括 `ext`）；`wrap` 会把原始值写进网关本地日志文件/stdout（用 `<E>...</E>` 标记以便下游 ES/ClickHouse 管道剥离，但网关自己的本地文件已经是明文）。这是刻意的单开关设计，不是疏漏——需要可读 path 时请在这两种权衡里二选一。同时注意 `access_log_options.include_raw_uri` 的交互：`replace` 默认下 `request_uri` 本身也会渲染为 `***`，仅开启该开关不再能得到可读的原始视图。

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
        max_files: 30
        check_interval_secs: 30
```

### 集群限流配置

当 Gateway 以多副本运行时，必须设置 `gateway_instance_count` 以确保限流计数准确：

```yaml
rate_limit:
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
Kubernetes/cgroup 内存限制保证。OIDC Access Token 缓存与数据面 `ExternalName`
DNS 缓存使用独立的 `lru_cache` 默认值；该默认值也是 DNS 缓存在 Service、主机名、
解析器变动下的容量上界。DNS 缓存只沿用容量，分片数另有下限——它在每个
`ExternalName` 请求上都会被读取，因此把 `default_shards` 设成低于该下限并不会
降低它的分片数。LDAP 认证不缓存。绝对安全上限分别为 65,536 个
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
  tls:
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
