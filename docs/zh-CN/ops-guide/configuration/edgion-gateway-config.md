# EdgionGatewayConfig

EdgionGatewayConfig 是 Edgion 的 Cluster 作用域 CRD（`edgion.io/v1alpha1`），用于设置 GatewayClass 级别的行为，通过 `GatewayClass.spec.parametersRef` 关联。更新会传播到已有 Gateway，无需重启进程；具体字段决定从哪个新请求、连接或操作开始采用更新。

## 与 Gateway YAML 的区别

| 配置层 | Gateway YAML | EdgionGatewayConfig |
|--------|-------------|---------------------|
| 作用域 | 单个 Gateway 进程 | 整个 GatewayClass |
| 变更生效方式 | 需要重启进程 | 动态生效，按字段的请求、连接或操作边界应用 |
| 典型用途 | 进程连接参数、日志路径、限流精度 | 超时策略、安全防护、全局插件、真实 IP 提取 |

**何时用 Gateway YAML，何时用 EdgionGatewayConfig：**
- 改 Controller gRPC 地址 → Gateway YAML `gateway.server_addrs`
- 改 Admin / Metrics 端口 → Gateway YAML（进程启动层）
- 配置 HTTP 超时、真实 IP、全局插件 → EdgionGatewayConfig

## 关联方式

通过 GatewayClass 的 `parametersRef` 字段关联 EdgionGatewayConfig：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config  # 对应下方 EdgionGatewayConfig 的 metadata.name
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config   # Cluster 作用域，无需指定 namespace
spec:
  server:
    enableCompression: false
    downstreamKeepaliveRequestLimit: 1000

  httpTimeout:
    client:
      readTimeout: "60s"
      writeTimeout: "60s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
      defaultIdleTimeout: "300s"

  maxRetries: 3

  realIp:
    trustedIps:
      - name: private-ranges
        cidrs: ["10.0.0.0/8", "172.16.0.0/12"]
    realIpHeader: "X-Forwarded-For"
    recursive: true

  securityProtect:
    xForwardedForLimit: 200
    requireSniHostMatch: true
    fallbackSni: ""
    tlsProxyLogRecord: true
    rejectDuplicateHost: true

  requestBody:
    enabled: true
    defaultMemoryBufferSize: "128KiB"
    # maxMemoryBufferSize: "1MiB"
    defaultMaxBodySize: "32MiB"
    # maxBodySize: "64MiB"
    storageOperationTimeout: "5s"

  globalPluginsRef:
    - name: "global-cors"
      namespace: "edgion-system"

  accessLogExtern:
    unmaskedKeys:
      header: [user-agent, origin, content-type, content-length, accept, x-request-id]
      respHeader: [content-type, content-length, cache-control, retry-after, x-request-id]
      query: []
      cookie: []
      ctx: []

  preflightPolicy:
    mode: "cors-standard"
    statusCode: 204
```

## 热更新生效边界

Edgion 会将被引用 EGC 的更新传播到已有 Gateway。这是 Edgion 的产品行为；
Gateway API 并未要求 GatewayClass 参数变更必须传播到已创建的 Gateway。

监听器持有共享配置句柄。EGC 更新时替换句柄内已解析的快照；每个新请求直接
捕获该快照，无需按 EGC 名称查表。删除和同名重建也会更新同一个句柄。

| 配置 | 新值生效时机 |
|---|---|
| HTTP 超时、重试、RealIp、XFF 与 SNI/Host 检查、重复 Host 检查、请求体、转发头、预检、压缩、HTTP 回环许可 | 每个新 HTTP 请求捕获一代配置，重试和响应处理继续使用该代配置 |
| `server.downstreamKeepaliveRequestLimit` | HTTP/1 首个请求捕获连接预算，同一连接后续请求继续使用该预算 |
| `tcpTimeout`、TCP/TLS 回环许可、`tlsProxyLogRecord` | 每条新 L4 代理连接捕获一次，重试、中继和日志继续使用该策略 |
| UDP 回环许可 | 每个新建 UDP 会话；已有会话保留原策略 |
| `fallbackSni` | 每次新 TLS 握手；已建立 TLS 连接不会重新选择证书 |
| 插件策略、全局插件、日志脱敏、LinkSys、负载均衡默认值、出站 TLS、DNS | 由各自已有的动态发布与消费机制按其作用域和生命周期更新 |

这些机制不构成跨资源种类或 Gateway 副本的原子更新。进行中的请求不会切换其捕获的
EGC 代次，但独立发布的共享服务仍遵循各自的更新语义。

删除被引用 EGC 后，新的请求和连接消费者恢复内置默认值；插件准入独立执行缺失引用的
fail-close 策略。修改 GatewayClass 引用本身、增加监听端口、转移共享监听器归属、
删除并重建 Gateway，均不属于此热更新契约。

## 配置参数

### spec.server

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enableCompression` | `Boolean` | `false` | 对带有效 `Content-Length` 且无 `Transfer-Encoding` 的适用下游响应启用 gzip；未知长度和 SSE 响应保持未压缩，以维持增量传输 |
| `downstreamKeepaliveRequestLimit` | `Integer` | `1000` | 下游每连接最大请求数，`0` 表示无限 |

压缩开关由每个 HTTP 请求捕获。keepalive 请求预算由 HTTP/1 首个请求捕获，并保留到
该连接结束。两者更新均无需重启监听器。Pingora
工作线程、关闭等待时间、上游 keepalive 池和 Pingora 错误日志路径仅属于本地 Gateway
YAML 的 `server` 配置，不属于 GatewayClass。

### spec.httpTimeout.client

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `readTimeout` | `Duration` | `60s` | 读取客户端请求的超时时间，覆盖 HTTP/1 与 HTTP/2 的每个请求体分块读取。计时器在每次收到分块后重置：停滞的上传会被释放，持续推进的上传不受总时长限制。注意：gRPC client-streaming/bidi 请求同样受此约束——若客户端消息间隔较长，请调大该值 |
| `writeTimeout` | `Duration` | `60s` | 写入客户端响应的超时时间 |
| `keepaliveTimeout` | `Duration` | `75s` | HTTP keepalive 连接空闲超时 |

### spec.httpTimeout.backend

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `defaultConnectTimeout` | `Duration` | `5s` | 连接上游服务器的超时时间 |
| `defaultRequestTimeout` | `Duration` | `60s` | 完整请求（含重试）的总超时时间 |
| `defaultIdleTimeout` | `Duration` | `300s` | 连接池中连接的空闲超时 |

### spec.tcpTimeout

L4 中继（纯 TCP、TLS 终止、TLS 透传）的超时配置。UDP 有独立的会话模型，不受本节影响。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `idleTimeout` | Duration 字符串 | `1h` | 在指定时间窗口内，若中继两个方向均无字节进展，则关闭该中继。任意方向完成一次读取或写入有所推进，计时器都会重置，因此活跃传输和缓慢但持续推进的传输不会被误断。 |
| `connectTimeout` | Duration 字符串 | `10s` | 建立上游 TCP 连接的上限（TCP 握手）。三类中继的上游连接都是纯 TCP，因此这里限制的是连接建立本身，而非 TLS 握手。当后端静默丢弃 SYN（防火墙、尚未就绪的 Pod）时快速失败，而不是把下游连接一直挂起到 OS TCP 重传超时（约 2 分钟）才结束。 |

**安全默认：** 如果省略整个 `tcpTimeout` 节，`1h` 空闲超时与 `10s` 连接超时仍然生效——无需任何配置，每条 L4 中继都有上限。

**动态更新：** 修改 `tcpTimeout` 无需重启监听器。TCP、TLS 终止或 TLS
透传连接进入 L4 代理处理时，会复制最新的已解析超时值。已建立连接继续使用建立时的值，
包括 TLSRoute 连接重试和后续中继过程。修改 GatewayClass 的 `parametersRef` 或共享物理
监听器的归属仍属于监听器生命周期变更，不会动态应用。

**关闭与风险：** 将 `idleTimeout` 设为 `"0s"`（任何值为零的合法 duration；裸 `"0"` 不是合法 GEP-2257 格式，会在 admission 时被拒绝）可禁用空闲超时（无界中继）。这会重新引入资源耗尽风险：一个停滞或低速发送的对端可以无限期持有中继——包括其 socket、任务和缓冲区——且进程内没有连接数上限。仅在上层机制已对连接生命周期设限的情况下才应禁用。同理，将 `connectTimeout` 设为 `"0s"` 会移除应用层连接上界，回退到 OS TCP 重传超时（Linux 上约 2 分钟）。

> **已知局限：** 空闲超时无法阻止有意为之的低速攻击者（每 N 秒发送一个字节，任一读写方向都能重置计时器）。要限制这类攻击需要总时长上限，但这会断开合法的长空闲连接（DB 连接池、消息队列、long-poll、SSH），不适合作为默认值。

```yaml
spec:
  tcpTimeout:
    idleTimeout: 1h
    connectTimeout: 10s
```

### spec.maxRetries

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `maxRetries` | `Integer` | `3` | 上游连接失败时的最大重试次数 |

### spec.realIp

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `trustedIps` | `Array<IpGroup>` | `[]` | 可信代理 IP 分组列表 |
| `realIpHeader` | `String` | `X-Forwarded-For` | 从哪个请求头提取真实 IP |
| `recursive` | `Boolean` | `true` | 是否从右向左遍历 header，跳过可信代理，取第一个非信任 IP |

`trustedIps` 中每一项都是一个 `IpGroup` 对象：`name`（必填；≤63 字符，字符集 `[a-zA-Z0-9._-]`，首尾必须是字母或数字，在列表内唯一）、`description`（可选）、`cidrs`（必填；非空的 IP 或 CIDR 字符串列表）。

提取逻辑：从右向左遍历 `realIpHeader`，跳过任一 `trustedIps` 分组中的地址，取第一个非信任 IP。

`spec.realIp` 在准入阶段按与 RealIp 插件相同的规则校验：`trustedIps` 为空、CIDR 格式错误或重复、`IpGroup` 名称超出 label-value 字符集，或 `realIpHeader` 不是合法的 HTTP header 字段名时，资源会给出 `Accepted=False` 与 `reason=Invalid`。在 Kubernetes 路径上，CRD 已带有 header 名称的 `pattern` 与 `maxLength`，格式错误的 `realIpHeader` 会被 API Server 直接拒绝。

Controller preparse 只告警、不阻止发布，因此 Gateway 侧做了同样的兜底校验：`spec.realIp` 非法时记录一条 error 日志，并按 `realIp` 未配置的方式运行（不做真实 IP 提取）。注意 `RealIpExtractor` 本身对单条 CIDR 是宽容的（跳过非法项并告警），所以此前会被静默降级的 CIDR 错误，现在会让整个 `realIp` 块失效。

### spec.securityProtect

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `xForwardedForLimit` | `Integer` | `200` | `X-Forwarded-For` 头部最大字节数，超过则返回 `400 Bad Request` |
| `requireSniHostMatch` | `Boolean` | `true` | HTTPS listener 级别 421 Misdirected Request 检测，SNI 与 Host 不匹配时拒绝请求 |
| `fallbackSni` | `String` | — | 客户端未提供 SNI 时使用的备用主机名 |
| `tlsProxyLogRecord` | `Boolean` | `true` | 是否记录 TLS 代理连接日志 |
| `allowLoopbackUpstream` | `Boolean` | `false` | 显式放行该 GatewayClass 的 loopback upstream；link-local / metadata 仍然拒绝 |
| `rejectDuplicateHost` | `Boolean` | `true` | 拒绝同一 HTTP/1.1 请求里出现多于一个 `Host:` 头部行，返回 `400 Bad Request`(RFC 9110 §7.2,Host smuggling 防御)。仅在需要兼容会发重复 `Host` 行的旧式中间件时关闭 |

省略整个 `securityProtect` 块时，不启用 XFF 字节上限；配置该块但省略
`xForwardedForLimit` 时，该字段默认为 200。

### spec.requestBody

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `Boolean` | `true` | 运行时请求体捕获开关；关闭后，需要请求体的插件按正常的 body 不可用结果执行 fail-close |
| `defaultMemoryBufferSize` | `String` | `128KiB` | 落盘前默认保留在内存中的字节数 |
| `maxMemoryBufferSize` | `String` | 未设置 | 内存溢写阈值的业务覆盖值；未设置时使用 `defaultMemoryBufferSize` |
| `defaultMaxBodySize` | `String` | `32MiB` | 完整请求体的默认上限 |
| `maxBodySize` | `String` | 未设置 | 请求体物理上限的业务覆盖值；未设置时使用 `defaultMaxBodySize` |
| `storageOperationTimeout` | `String` | `5s` | 单次落盘请求体操作的非零时限，同时覆盖等待存储 I/O 准入和执行文件操作 |

所有大小都必须是正数可读字节值，有效内存阈值不能超过有效请求体上限。Gateway 在发布配置更新时解析这些值；每个新请求从不可变快照复制已解析策略，不会解析大小字符串或获取阻塞锁。插件的 `body.maxBodySize` 可以进一步收紧该插件的限制。

`storageOperationTimeout` 使用共享的 GEP-2257 duration 格式（如 `500ms`、`5s`、`1m`），且必须大于零。超过该时限会报告为 Gateway 本地存储故障，而不是下游请求体读取超时。捕获失败且仍可能存在未读 body 时，HTTP/1 连接不可复用；HTTP/2 只重置受影响的 stream。

### spec.globalPluginsRef

应用于该 GatewayClass 下所有路由的全局插件列表，在路由级插件之前执行。

```yaml
globalPluginsRef:
  - name: "global-cors"       # EdgionPlugins 资源名称
    namespace: "edgion-system" # 默认 "default"
```

### spec.accessLogExtern

[`AccessLogExtern`](../observability/access-log.md#扩展日志accesslogextern) 声明字段的脱敏白名单。平台方所有——是 Cluster 作用域，与 `globalPluginsRef` 同级——因此某个 namespace 下自己的 `EdgionPlugins` 无法为整个集群放宽脱敏。

```yaml
accessLogExtern:
  unmaskedKeys:
    header:     [user-agent, origin, content-type, content-length, accept, x-request-id]
    respHeader: [content-type, content-length, cache-control, retry-after, x-request-id]
    query:      []
    cookie:     []
    ctx:        []
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `unmaskedKeys.header` | `[]String` | `[user-agent, origin, content-type, content-length, accept, x-request-id]` | 明文记录的请求头名称（大小写不敏感） |
| `unmaskedKeys.respHeader` | `[]String` | `[content-type, content-length, cache-control, retry-after, x-request-id]` | 明文记录的响应头名称（大小写不敏感） |
| `unmaskedKeys.query` | `[]String` | `[]` | 明文记录的 query 参数名（大小写敏感） |
| `unmaskedKeys.cookie` | `[]String` | `[]` | 明文记录的 cookie 名（大小写敏感） |
| `unmaskedKeys.ctx` | `[]String` | `[]` | 明文记录的 ctx 变量名（大小写敏感） |

白名单按来源类型分开维护，而非一张扁平列表——名为 `user-agent` 的请求头是无害的，但同名的 cookie 是完全不同的对象。`query`/`cookie`/`ctx` 默认为空，因为它们的 key 名称完全由业务决定，不存在放之四海皆安全的默认名。未命中白名单的字段仍会输出，只是被脱敏，而非直接丢弃。`routeLabel`/`routeAnnotation` 来源永远不脱敏，完全绕过该白名单。凭证类请求头、claim/payload 类 ctx key 等硬黑名单无论此白名单如何配置都无法被命名。其执行分两步：在 `EdgionPlugins` 准入阶段，声明了被拉黑 key 的字段只会被**上报**到该资源的 `Accepted=False` status condition 上（本仓库的准入是宽松的——上报并不会把资源挡在集群之外，它仍会进入 controller 缓存并通过 conf_sync 下发给 gateway）；真正的强制生效发生在 **gateway 加载配置时**，由 `compile_plugins` 的字段 `retain()` 完成——它会在 spec 被存入之前，丢弃每个校验错误不为空的已声明字段。扩大黑名单需要改代码，而不是改 CR。

**某个 `unmaskedKeys` 列表缺省，保留的是该列表的默认值，而不是清空它。** `UnmaskedKeys` 会为每个缺省的列表分别填充其自身的默认值，因此 `unmaskedKeys: {ctx: [tenant_id]}` 并不代表「只有 `tenant_id` 明文」——它同时保留了上面 `header` 和 `respHeader` 的默认列表。若只想明文 `tenant_id` 并去掉 header 默认值，需要显式列出每个类别，例如 `header: []`。

脱敏策略与逻辑 Gateway 选中的插件运行时作用域一起发布。其发布独立于监听器的
请求／连接 EGC 快照；共享监听器归属不会替代该插件作用域。

### spec.preflightPolicy

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `mode` | `String` | `cors-standard` | `cors-standard`：选入带 `Origin` + `ACRM` 的 OPTIONS 请求；`all-options`：将所有 OPTIONS 请求选入全局处理。路由 CORS filter 仍决定是否在本地响应。详见 [Preflight 策略](../gateway/preflight-policy.md)。 |
| `statusCode` | `Integer` | `204` | 无 CORS 插件时 preflight 最终响应的状态码（200-599） |

### spec.loadBalancing

`degradeThreshold` 是 0 到 100 的百分比，默认为 0（禁用）。当健康端点比例低于阈值时，
降级模式绕过健康检查和异常剔除过滤，在所有端点之间进行负载均衡，因此可能向不健康的后端
发送流量。此功能仅在启用了健康检查或异常检测时生效。

`EdgionBackendTrafficPolicy.spec.loadBalancer.degradeThreshold` 可为目标 Service 覆盖全局默认值，
包括用显式的 0 禁用降级。HTTP/gRPC 请求在首次上游尝试时捕获 Service 覆盖值，后续重试在策略
更新或删除后仍保留该值。未捕获覆盖值的请求继续使用当前全局默认值，即使随后新增了 Service
覆盖值也不会采用它。全局默认值独立更新，仍可能影响这类请求。端点成员与健康状态保持实时。
TCP/UDP/TLS 和请求镜像在每次选择端点时使用当前 Service 覆盖值或全局默认值。

## 常见配置场景

### 配置真实 IP 提取

当 Gateway 部署在负载均衡器后面时，配置可信代理 IP 以正确提取客户端真实 IP：

```yaml
spec:
  realIp:
    trustedIps:
      - name: private-ranges
        cidrs:
          - "10.0.0.0/8"
          - "172.16.0.0/12"
          - "192.168.0.0/16"
    realIpHeader: "X-Forwarded-For"
    recursive: true
```

### 调整超时策略

针对慢接口场景放宽超时：

```yaml
spec:
  httpTimeout:
    client:
      readTimeout: "120s"
      writeTimeout: "120s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "10s"
      defaultRequestTimeout: "120s"
      defaultIdleTimeout: "300s"
```

### 挂载全局 CORS 插件

```yaml
spec:
  globalPluginsRef:
    - name: "global-cors"
      namespace: "edgion-system"
  preflightPolicy:
    mode: "cors-standard"
    statusCode: 204
```

### 开启下游响应压缩

```yaml
spec:
  server:
    enableCompression: true
```

### 关闭 SNI 主机名匹配检测

在某些特殊场景下，允许 SNI 与 Host 头不一致：

```yaml
spec:
  securityProtect:
    requireSniHostMatch: false
```

## 相关文档

- [Controller 配置](./controller-config.md)
- [Gateway 配置](./gateway-config.md)
- [GatewayClass 配置](../gateway/gateway-class.md)
- [Preflight Policy](../gateway/preflight-policy.md)
