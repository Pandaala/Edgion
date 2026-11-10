# EdgionGatewayConfig

## 下游 TLS 默认策略

`EdgionGatewayConfig.spec.downstreamTls` 为绑定到该 GatewayClass 配置的 TLS 终结监听器提供默认策略，
同时适用于原生 Gateway 证书和 EdgionTls 证书：

```yaml
spec:
  downstreamTls:
    minTlsVersion: TLS1_2
    ciphers: [ECDHE-RSA-AES128-GCM-SHA256, ECDHE-ECDSA-AES128-GCM-SHA256]
    ecdhCurves: [P-256, P-384]
```

| 字段 | 继承与覆盖规则 |
|---|---|
| `minTlsVersion` | EdgionTls 显式值优先，其次为此默认值，最后为后端默认值。局部 `TLS1_2` 可覆盖全局 `TLS1_3`；`TLS1_0`/`TLS1_1` 仍是已弃用的显式选择。 |
| `ciphers` | 优先级：EdgionTls 显式列表 → EGC 列表 → FIPS/后端默认值。仅当两处字段都缺省时才继承 FIPS/后端默认值。任一层的显式 `[]` 都表示使用后端默认套件，包括启用 FIPS 策略时。仅影响 TLS 1.2 及更早版本。 |
| `ecdhCurves` | 由 `X25519`、`P-256`、`P-384`、`P-521` 组成的有序列表，长度为 1–4 且不能重复。EdgionTls 和 listener 均没有曲线覆盖字段。 |

启用 FIPS 策略时，缺省曲线为 `P-256/P-384/P-521`；显式 NIST 子集保留其顺序，
包含 `X25519` 则拒绝握手。密码套件覆盖不会绕过曲线限制。未启用 FIPS 策略时，
缺省曲线采用后端默认值。空列表、重复名称和不支持的曲线均无效。

实际生效的密码套件配置无效时拒绝握手，不回退到更宽松的默认值；有效的局部密码套件
覆盖可屏蔽无效的继承列表。Controller 验证传输安全语法，Gateway 的 BoringSSL 在应用
实际策略时验证套件表达式。发布前被拒绝的更新保留此前已接收的配置代际。

新握手捕获一个 EGC 配置代际并在协议协商前应用策略；已建立连接保留协商结果。
物理端口保留启动时所属的 EGC 绑定，热更新不会重新绑定 GatewayClass 引用或转移共享监听器所有权。


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
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Cluster
  data:
    type: IpList
    config:
      items:
      - name: private-ranges
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
---
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
    downstreamKeepaliveRequestLimit: 10000

  httpTimeout:
    client:
      readTimeout: "60s"
      writeTimeout: "60s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
      defaultIdleTimeout: "300s"

  retry:
    attempts: 2

  realIp:
    trustedIpRefs:
      - name: realip-proxies-1
        namespace: default
    realIpHeader: "X-Forwarded-For"
    recursive: true

  securityProtect:
    xForwardedForLimit: 200
    requireSniHostMatch: true
    fallbackSni: ""
    tlsProxyLogRecord: true

  requestBody:
    enabled: true
    maxBodySize: "32MiB"
    storage:
      memoryLimit: "128KiB"
      spillToDisk: true
      # maxBufferedSize: "32MiB"
      operationTimeout: "5s"

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
| `downstreamKeepaliveRequestLimit` | `Integer` | `10000` | 每条下游 HTTP/1 连接的最大请求数，`0` 表示无限 |

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

### spec.retry

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `attempts` | `Integer` | `2` | 首次上游尝试后的最大重试次数；`0` 表示禁用重试，支持范围为 `0..=2147483647` |

首次上游请求不消耗此重试预算，因此上游请求总数最多为 `1 + attempts`。
路由级 `rules[].retry.attempts` 会覆盖此 GatewayClass 全局默认值。

### spec.realIp

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `trustedIpRefs` | 必填 | 非空、有序的 IpList 引用，每项必须包含 `name` 和显式 `namespace` |
| `realIpHeader` | `X-Forwarded-For` | 有效 HTTP 字段名，不超过 256 字节 |
| `recursive` | `true` | 从右到左遍历有效跳点；全部可信时返回最左侧有效跳点 |
| `maxTrustedHops` | 无限制 | 递归跳数上限；0 在首个可信跳点停止；非递归模式忽略此值 |

IP/CIDR 集合存储在 IpList 的 `items`（IpGroup）中。启用 ReferenceGrant 校验时，全局 EGC 只能读取当前为 `Cluster` 可见的列表；命名空间级授权和出站 TLS 的管理权限例外不适用。关闭校验仍检查资源身份、类型及启用状态。

全部引用可用后才信任 PROXY 协议处理后的对端和转发链。列表缺失、禁用、未授权或类型错误时，本次解析使用连接对端。一次快照覆盖所有跳点；列表内容、可见性、删除及恢复在后续解析中生效，不复制 CIDR，也不重建监听器。中央编译器保留有效条目并排除无效条目；空列表或全部无效的列表不信任任何地址。

实际进入类型化发布的配置若存在固有错误，会报告 `Accepted=False` 并关闭全局提取。Kubernetes 的必填项、最小数量、命名空间及请求头约束也可能直接在 API admission 拒绝更新；该更新不会到达 Controller/Gateway，上一份已获准的配置保持有效。混合旧内联 `trustedIps` 或未知字段会被窄范围保留，并使进入类型化发布的配置失效。CRD 仅在 realIp 对象内保留未知字段，让类型化无效状态替换旧运行时，而不是静默剪除旧字段。干净的新一代配置可恢复。共享 EGC 状态仅描述固有结构，不写入部署本地的列表可用性；列表可用性在消费该配置的 GatewayClass 上报告为 `ResolvedRefs=False`。非递归模式使用最右侧有效跳点（若其不可信），否则使用连接对端。

### spec.securityProtect

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `xForwardedForLimit` | `Integer` | `200` | `X-Forwarded-For` 头部最大字节数，超过则返回 `400 Bad Request` |
| `requireSniHostMatch` | `Boolean` | `true` | HTTPS listener 级别 421 Misdirected Request 检测，SNI 与 Host 不匹配时拒绝请求 |
| `fallbackSni` | `String` | — | 客户端未提供 SNI 时使用的备用主机名 |
| `tlsProxyLogRecord` | `Boolean` | `true` | 是否记录 TLS 代理连接日志 |
| `allowLoopbackUpstream` | `Boolean` | `false` | 显式放行该 GatewayClass 的 loopback upstream；link-local / metadata 仍然拒绝 |

Pingora 在协议解析阶段拒绝包含多个 `Host` 头部字段的 HTTP/1.1 请求；网关没有用于关闭此校验的配置项。

省略整个 `securityProtect` 块时，不启用 XFF 字节上限；配置该块但省略
`xForwardedForLimit` 时，该字段默认为 200。

### spec.requestBody

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `Boolean` | `true` | 运行时请求体捕获开关；关闭后，需要请求体的插件按正常的 body 不可用结果执行 fail-close |
| `maxBodySize` | `String` | `32MiB` | 请求体硬上限 |
| `storage.memoryLimit` | `String` | `128KiB` | 落盘前的内存 payload 阈值 |
| `storage.spillToDisk` | `Boolean` | `true` | 超过内存阈值后允许落盘 |
| `storage.maxBufferedSize` | `String` | 继承 maxBodySize | 新缓存 owner 的保留容量上限 |
| `storage.operationTimeout` | `String` | `5s` | 单次存储操作的准入及执行时限 |

必须满足 `memoryLimit <= maxBufferedSize <= maxBodySize`，省略缓存上限时继承请求体上限。关闭落盘后，新完整缓存的容量还受内存阈值约束；超过容量会失败，不会返回不完整的 body。新 owner 的容量也约束后续正文修改；已有完整缓存保留原 owner 的容量。显式 WAF raw-prefix 使用独立的至多 1MiB 内存窗口。只有 WAF 需要正文时，full 检查可以在配置的检查或存储边界停止，由 `onLimit` 选择继续或拒绝；继续转发不会截断正文。其他需要完整正文的插件仍要求完整缓存。

普通 HTTP 即使没有 WAF 或正文插件、或关闭捕获，也执行物理上限。已知 Content-Length 超限时在转发前拒绝；未知长度则按实际接收字节计数终止上传。已经选定的响应保留原有响应权，不再替换为 413。gRPC、CONNECT 和成功升级后的隧道除外，仅携带 Upgrade 请求头不能绕过限制。

所有大小都必须是正数可读字节值，有效内存阈值不能超过有效请求体上限。Gateway 在发布配置更新时解析这些值；每个新请求从不可变快照复制已解析策略，不会解析大小字符串或获取阻塞锁。插件的 `body.maxBodySize` 可以进一步收紧该插件的限制。

`storage.operationTimeout` 使用共享的 GEP-2257 duration 格式（如 `500ms`、`5s`、`1m`），且必须大于零。超过该时限会报告为 Gateway 本地存储故障，而不是下游请求体读取超时。捕获失败且仍可能存在未读 body 时，HTTP/1 连接不可复用；HTTP/2 只重置受影响的 stream。

### spec.globalPluginsRef

应用于该 GatewayClass 下所有路由的全局插件列表，在路由级插件之前执行。

```yaml
globalPluginsRef:
  - name: "global-cors"       # EdgionPlugins 资源名称
    namespace: "edgion-system" # 默认 "default"
```

若引用的 `EdgionPlugins` 资源不存在，将按 fail-closed 处理：本应执行该 GatewayClass
全局插件的请求在到达上游之前会被拒绝并返回 `500`，而不是在缺少该插件的情况下继续放行。
Gateway 上的 `edgion.io/edgion-plugins` 注解遵循同样的规则（作用于该 Gateway 的请求）。
在只由单个 Gateway 使用的 listener 上，拒绝发生在路由匹配之前，因此该 listener 上的
ACME HTTP-01 校验请求也会被拒绝。请先创建 `EdgionPlugins` 资源再引用它；删除资源前先移除引用。目标缺失会在消费该配置的 GatewayClass 上报告为 `ResolvedRefs=False`（见 GatewayClass 页面）。

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
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: realip-proxies-1
  namespace: default
spec:
  visibility: Cluster
  data:
    type: IpList
    config:
      items:
      - name: private-ranges
        cidrs:
        - 10.0.0.0/8
        - 172.16.0.0/12
        - 192.168.0.0/16
---
spec:
  realIp:
    trustedIpRefs:
      - name: realip-proxies-1
        namespace: default
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

## 后端 DNS 地址族默认值

```yaml
spec:
  dnsResolver:
    lookupFamily: ipv4Only
```

`servers` 的每一项必须是 `ip`、`ip:port` 或带方括号的 IPv6 地址（默认端口 53）；
其他写法（包括在一个字符串里用逗号写多个地址）会使 EGC 变为 `Accepted=False`。

`lookupFamily` 可选 `ipv4Only`（默认）、`ipv6Only`、`all`，无需同时配置服务器或
`linkSysRef`。现有全局 EGC 选择规则决定发布者，地址族与解析器一起热更新。ExternalName
Service 注解优先覆盖；AI 外部域名使用 Gateway 覆盖再继承 EGC。`controller` 模式的 Service
仍由 Controller/Service 策略控制。`all` 不提供连接竞速，IPv6 需要实际可用的上游网络。

DNS 默认值沿用现有全局规则：在定义 DNS 配置的 EGC 中选择 store key 最小者，
并非当前 GatewayClass 独占的配置。只有 `lookupFamily` 的 DNS 配置块也参与选择。

## 后端 DNS 端点表

`gateway` 模式（默认，见 [ExternalName 解析](../../user-guide/http-route/backends/service-ref.md#externalname-解析)）
的 ExternalName Service 由每个 Gateway 内的后台端点表解析。返回的每个地址都成为普通的
负载均衡端点，适用于 HTTP、gRPC、TCP、UDP 和 TLS 路由。

与上面的解析器和地址族默认值一样，这些设置在整个进程内生效：一个 Gateway 进程只使用
所有定义了 `dnsResolver` 的 EGC 中名称最小的那个 EGC 的配置，与哪个 GatewayClass 引用它
无关。请只在一个 EGC 中定义 `dnsResolver`；其他 EGC 中的配置块（即使只设置了地址族）
可能在不报错的情况下取代它。

```yaml
spec:
  dnsResolver:
    endpointTable: true          # false：未加注解的 ExternalName Service 使用操作系统解析器
    ttlFactor: 0.5               # 刷新间隔 = clamp(ttl * ttlFactor, min, max)
    minRefreshSeconds: 5         # 最小 2
    maxRefreshSeconds: 300       # 最大 2147483647（DNS TTL 上限）
    # fallbackRefreshSeconds: 5  # 无 TTL 时使用；不设置 = 5 并收敛到 [min, max]
    # refreshIntervalSeconds: 30 # 固定间隔，忽略记录 TTL
    maxStaleSeconds: 300
    maxEntries: 4096
```

| 字段 | 默认值 | 说明 |
|---|---|---|
| `endpointTable` | `true` | 未设置 `edgion.io/dns-resolution-mode` 的 ExternalName 默认模式：`true` 为 `gateway`，`false` 为 `system`。 |
| `ttlFactor` | `0.5` | 按 `ttl x ttlFactor` 刷新，取值 0.01–1。 |
| `minRefreshSeconds` | `5` | 最短刷新间隔，最小 2。 |
| `maxRefreshSeconds` | `300` | 最长刷新间隔，最大 2147483647（DNS TTL 上限）。 |
| `fallbackRefreshSeconds` | 不设置（5） | 解析器不返回 TTL 时（操作系统解析器、无 `ttlPath` 的 HTTP DNS）的刷新间隔。显式设置时必须位于 min 与 max 之间；不设置时取 5 并收敛到 [min, max]，因此只改上限或下限也不会失效。 |
| `refreshIntervalSeconds` | 不设置 | 固定刷新间隔，设置后忽略记录 TTL 和 min/max 上下限（最低 2 秒）。 |
| `maxStaleSeconds` | `300` | 解析开始失败后保留上次地址的时长。 |
| `maxEntries` | `4096` | 不同主机名 / 解析器 / 地址族组合的上限；超出后新的目标没有端点。 |

时长字段使用整数秒，因为 DNS TTL 上限（2147483647 秒，RFC 2181）无法用时长字符串表示。
不一致的配置（例如 `minRefreshSeconds` 大于 `maxRefreshSeconds`，或 `fallbackRefreshSeconds`
不在两者之间）在 Kubernetes 中由准入校验拒绝，其他模式下标记为 `Accepted=False`；Gateway
收到这样的配置时记录错误日志，并使用默认刷新设置（保留 `endpointTable`）。修改刷新设置会
重新调度已有条目，缩短的间隔立即生效。

- 主机名、解析器和地址族都相同的 Service 共用一个条目、一次查询。条目随最后一个引用它的
  Service 一起删除，Service 删除或主机名变更都不会残留端点。
- 解析结果排序后比较，结果不变时不做任何更新，健康检查和异常剔除状态在刷新后保留。
- 解析失败时，从第一次刷新失败起保留上次地址 `maxStaleSeconds`。重试从 `minRefreshSeconds`
  开始倍增，最长 60 秒（不超过 `maxRefreshSeconds`，也不低于 `minRefreshSeconds`）。权威空
  结果立即撤下端点。操作系统解析器无法区分 NXDOMAIN 和故障，因此已删除的域名会保留到
  `maxStaleSeconds`。
- 修改 Service（或默认）的解析器或地址族时，在新解析器返回第一个结果之前继续使用上次的
  结果（只保留新地址族的地址），因此切换到可用的解析器不会造成端点空窗。如果第一个结果是
  失败，则撤下端点：错误的新配置会立即生效而不会被旧结果掩盖，也不会再用旧解析器查询。
  修改主机名则立即撤下旧地址。
- 从未解析成功的目标，HTTP 请求或 TCP/TLS 连接最多等待 2 秒的首次解析。UDP 不等待：
  首次解析完成前到达的数据报被丢弃，由客户端重传。
- 链路本地地址（169.254.0.0/16、fe80::/10）和未指定地址（0.0.0.0、::）会被丢弃，不会成为
  端点或健康检查目标。
- 每次刷新加 ±10% 抖动。按 TTL 计算的刷新保持在 min/max 范围内；固定的
  `refreshIntervalSeconds` 只保证不低于 2 秒。
- Gateway 管理 API `GET /api/v1/dns/endpoint-table` 可查看条目、地址、状态和引用的 Service。

## 按请求的 DNS 应答缓存

AI 供应商域名以及 RegionRoute、DynamicExternalUpstream 插件的外部目标在每个请求中才确定，
因此使用一个有界应答缓存，而不是端点表。`system` 模式 ExternalName Service 的新 UDP 会话
也通过这个缓存解析（操作系统解析器，每个数据报最多等待 250 毫秒），避免大量新客户端地址
放大 DNS 查询；`system` 模式的 HTTP、gRPC、TCP 和 TLS 流量不经过该缓存，每个请求或连接都
直接询问操作系统。它们使用这里的解析器和 `lookupFamily` 默认值
（或 Gateway 的覆盖值）；IP 字面量目标按原样使用。每个应答最多保留 256 个地址，按解析器、
域名和地址族区分，每次使用前仍执行上游地址安全检查。

- 应答在解析器报告的记录 TTL 内保持新鲜，TTL 限制在 5–300 秒。Standard DNS 服务器会报告
  记录 TTL；HTTP DNS 在 JSON 响应格式配置了 `ttlPath` 时报告（`aliyun` 预设已配置）。
- `fallbackRefreshSeconds`（生效值）是解析器未报告 TTL 时（例如系统解析器）的新鲜期，
  在这个缓存里最多 300 秒。因此调大 `minRefreshSeconds` 也会拉长它；该缓存无法关闭。
- 应答过期后 30 秒内仍会返回，同时由一个请求刷新。解析器故障时，只要仍被请求，就继续使用
  上次的有效应答，但最多超过新鲜期 5 分钟。故障期间的刷新从 1 秒指数退避到 10 秒。
- 没有可回退应答的失败查询缓存 1 秒，连续失败时倍增，最长 10 秒；同一冷域名的并发查询
  只发起一次解析。

请求会在解析出的地址之间轮转起始地址，使流量分散到所有地址，单个故障地址不会接住全部
请求。AI 请求的后续尝试还会优先选择本请求尚未连接过的地址。
