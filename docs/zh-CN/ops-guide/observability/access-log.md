# Access Log

> **🔌 Edgion 扩展**
>
> access log 格式和插件日志结构是 Edgion Gateway 的特有功能，不属于标准 Gateway API 规范。

Edgion Gateway 为每个请求生成一条结构化 JSON access log，记录完整的请求生命周期：客户端信息、路由匹配、后端响应、每个插件的执行日志和错误。这是网关诊断单个请求的主要机制。

## 概述

access log 是回答*特定请求*相关问题的正确工具——"带 `x-trace-id=abc` 的请求发生了什么？"。聚合指标（见[监控指标](./metrics.md)）回答集群级问题；access log 回答单请求级问题。

每条日志是一行上的单个 JSON 对象，文件可直接被 `jq`、Loki、Elasticsearch 或任何面向行的日志管道消费。

```
client request ──► Gateway ──► backend
                     │
                     └──► one JSON line ──► logs/edgion_access.log
```

## 配置

access log 在网关配置的 `access_log` 段下配置。

### 最小配置

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

此配置使用默认队列和轮转行为启用日志。`path` 除非为绝对路径，否则相对于 `work_dir` 解析。

### 生产配置

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # 反压作用于请求路径之前的内存队列
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | Boolean | `false` | 启用 access log 通道 |
| `output.localFile.path` | String | none | 日志文件路径（除非为绝对路径，否则相对于 `work_dir`）。使用 `localFile` 时**必填** |
| `output.localFile.queue_size` | Integer | none | 反压生效前的内存队列大小 |
| `output.localFile.rotation.strategy` | Enum | none | 轮转策略。示例值：`daily` |
| `output.localFile.rotation.max_files` | Integer | none | 保留的轮转文件最大数量 |
| `output.es` | Object | none | 将 access log 发送到 Elasticsearch（通过 `link_sys`）。设置 `linkSysRef.namespace` 和 `linkSysRef.name` 以引用目标 LinkSys 资源 |

原始客户端入口 URI（`request_uri`）由单独的顶层 `access_log_options` 段控制，而非 `access_log` 块——见[特殊字段](#特殊字段)。

> `output` 块也接受 `es` sink。当 Elasticsearch 生产者→批量循环通道被填满时，`edgion_link_sys_es_bulk_channel_full_total` 计数器递增——将其作为反压信号监控。见[监控指标](./metrics.md)。

## 降级：fallback sink 与手动回填

### Fallback sink

`access_log` 块在 `output` 之外支持可选的 `fallback` 字段。当主 sink 的 `healthy()` 检查返回 false（例如 Elasticsearch 或 Kafka 故障期间），新条目将被路由到 fallback。每条日志**最多落入一个 sink**——不存在扇出分发。

典型模式：Elasticsearch 作为主 sink，有界 LocalFile 作为 fallback，无需额外基础设施即可在故障期间保留 access log：

```yaml
access_log:
  enabled: true
  output:
    es:
      linkSysRef:
        namespace: default
        name: es-main
  fallback:
    localFile:
      path: logs/es_access_fallback.log
      rotation:
        strategy: { size: 104857600 }   # 每文件 100 MB
        maxFiles: 5                       # 硬上限约 500 MB
      diskUsageThresholdPercent: 90
```

**限制 fallback 磁盘用量（推荐）。** 长时间故障期间无限制的 fallback 文件可能撑满磁盘。结合 `rotation.strategy: { size: <字节数> }` 和 `rotation.maxFiles`（硬上限 ≈ size × maxFiles）限制增长。`diskUsageThresholdPercent`（默认 90）会在宿主卷用量超过该百分比时，让 sink 将自身标记为不健康并丢弃条目——当 fallback 本身是链路中最后一个 sink 时，此为最后防线。如需禁用磁盘检查，设置 `diskUsageThresholdPercent: 0`。

### Fallback 不会自动回填

故障期间落入 fallback LocalFile 的条目会保留在文件中。主 sink 恢复后，网关自动将新条目路由回它，但 fallback 文件**不会**自动重放。如需恢复遗漏的记录，需手动触发回填。

### 回填接口

```
POST /api/v1/access-log/backfill
Authorization: Bearer <admin-token>
```

此接口与其他 `/api/v1/` 管理接口一样，需要 admin bearer token。

**前置条件** — 任意条件不满足时接口立即返回错误：

| HTTP 状态码 | 条件 |
|-------------|------|
| `400` | 主 sink 不是 Elasticsearch |
| `400` | 未配置 LocalFile fallback |
| `409` | Elasticsearch 当前不健康——等待恢复后重试 |
| `503` | Access logger 未初始化 |

**成功响应（`200`）**

```json
{
  "success": true,
  "data": {
    "summary": {
      "files_processed": 2,
      "lines_backfilled": 41,
      "lines_failed": 0,
      "files_remaining": 0,
      "stopped_reason": null
    }
  }
}
```

**行为详情**

- 逐行读取每个 fallback 文件（每行为一条 ES 文档），通过常规 bulk 通道重新写入 Elasticsearch。
- 处理完一个文件后，将其重命名为 `<name>.backfilled-<epoch_secs>`。后续触发会跳过已重命名的文件，使操作在**文件粒度上幂等**。
- **至少一次投递（重要警告）**：故障边界处正在处理的批次可能在 Elasticsearch 中出现重复——不使用稳定的文档 ID，不执行去重。Access log 可以容忍此级别的重复，但在查询聚合计数时需注意。
- **索引归属**：回填的文档会写入**当前日期**的索引（`<prefix>-<今天>`），而不是请求实际发生的日期——回填走与实时流量相同的 bulk 路径，索引名由当前时间推导。每个文档仍带有自己的 `@timestamp`，因此用 `<prefix>-*` 索引模式做时间范围查询仍能查到；只有按具体日期索引查询、或按日索引做保留/ILM 的场景会受影响。
- 接口通过现有 ES bulk 通道写入，具有自然背压。建议在**业务低峰期**触发，避免在实时流量之上叠加额外的批量写入压力。

### 回填指标

两个计数器用于跟踪回填进度，补充现有的 `edgion_link_sys_es_bulk_channel_full_total` 背压指标：

| 指标 | 说明 |
|------|------|
| `edgion_access_log_backfill_ingested_total` | 回填时重新写入 Elasticsearch bulk 通道的行数（最终落库由后台 bulk loop 异步完成） |
| `edgion_access_log_backfill_failed_total` | 无法写入 bulk 通道的行数（通道已关闭）；该 fallback 文件会保留以便后续重试 |

## 日志格式

日志始终是 JSON，每行一条。可选字段为空时完全省略（而非输出 `null`），保持每行紧凑。顶层结构如下：

```json
{
  "ts": 1767089146376,
  "request_info": {
    "direct_peer_ip": "192.168.1.20",
    "host": "api.example.com",
    "path": "/health",
    "status": 200
  },
  "match_info": { },
  "backend_context": { },
  "genai": { },
  "stage_logs": [ ],
  "info": [ ],
  "errors": [ ]
}
```

请求作用域字段（`direct_peer_ip`、`host`、`path`、`status` 等）嵌套在 `request_info` 对象内、下沉一层；路由、后端和插件信息与它一起并列在顶层。

## 字段参考

### 顶层字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `ts` | Integer | 条目时间戳，纪元毫秒 |
| `request_info` | Object | 请求作用域字段——见下方[请求字段](#请求字段) |
| `match_info` | Object | 匹配的路由标识（无路由匹配时省略） |
| `backend_context` | Object | 后端标识和上游时序（无后端时省略/`null`） |
| `genai` | Object | 最终 GenAI 用量与有界 per-attempt 核算（非 AI 请求省略） |
| `stage_logs` | Array\<Object\> | 按阶段分组的每插件执行日志（为空时省略） |
| `info` | Array\<String\> | 网关内部信息事件（为空时省略） |
| `errors` | Array\<String\> | 处理过程中捕获的错误原因（为空时省略） |
| `ext` | Object | 来自 [`AccessLogExtern`](#扩展日志accesslogextern) 的操作员声明字段（未采集到任何字段时省略） |

### 请求字段

嵌套在顶层 `request_info` 对象内、下沉一层（见上方[顶层字段](#顶层字段)），而非平铺在条目顶层。每个字段不存在时省略。

| 字段 | 类型 | 说明 |
|------|------|------|
| `direct_peer_ip` | String | 下游物理传输对端（`DirectPeerIp`） |
| `client-port` | Integer | 客户端 TCP 端口 |
| `remote_ip` | String | 经过适用的 PROXY/RealIp 解析后的客户端 IP（`RemoteIp`） |
| `x-trace-id` | String | 每请求追踪 ID；客户端传递相同值可关联日志 |
| `host` | String | 请求 `Host` 头 |
| `path` | String | 规范化后的请求路径 |
| `status` | Integer | 响应状态码 |
| `x-forwarded-for` | String | `X-Forwarded-For` 头值（存在时） |
| `discover_protocol` | String | 自动检测的协议。示例：`grpc`、`websocket` |
| `grpc_service` | String | gRPC 服务名（仅 gRPC 请求） |
| `grpc_method` | String | gRPC 方法名（仅 gRPC 请求） |
| `sni` | String | TLS 服务器名指示（仅 TLS 连接） |
| `tls_id` | String | TLS 连接/会话标识符（仅 TLS 连接） |
| `request_uri` | String | 客户端入口原始 URI；**仅当** `access_log_options.include_raw_uri: true` 时输出 |

### `match_info` 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `rns` | String | 路由命名空间 |
| `rn` | String | 路由名称 |
| `rule_id` | Integer | 匹配的规则索引（从 0 开始） |
| `match_id` | Integer | 匹配的匹配项索引（从 0 开始） |
| `sv` | Integer | Controller→Gateway 同步版本；为 0 时省略 |

无路由匹配时，整个 `match_info` 对象省略。

### `backend_context` 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | String | 后端服务名称 |
| `namespace` | String | 后端服务命名空间 |
| `upstreams` | Array\<Object\> | 每次上游尝试的条目（重试时产生多条） |

#### `upstreams[]` 子字段

| 字段 | 类型 | 单位 | 说明 |
|------|------|------|------|
| `ip` | String | — | 上游 IP 地址 |
| `port` | Integer | — | 上游端口 |
| `status` | Integer | — | 上游响应状态码 |
| `ct` | Integer | ms | 连接时间 |
| `ht` | Integer | ms | 首字节时间（到响应第一字节） |
| `bt` | Integer | ms | 响应体首字节时间 |
| `et` | Integer | ms | 本次尝试的总耗时 |
| `err` | Array\<String\> | — | 本次尝试的错误信息（为空时省略） |

无后端使用时（路由未匹配，或请求被插件短路），`backend_context` 为 `null` 或省略。

### `genai` 字段

最终请求汇总包含 `provider`、`model`、`input_tokens`、`output_tokens`、可选的 cache token 计数与
`estimated_cost_usd`、`streaming`、`usage_estimated`、可选的 `quota_status` 与 `ttft_ms`，以及下方的
per-attempt 字段。`attempts` 最多包含八次已派发尝试，其序列化段上限为 8 KiB。当为遵守上述任一边界而
省略后续汇总时，会出现 `attempts_truncated: true`。

| `attempts[]` 字段 | 类型 | 说明 |
|--------------------|------|------|
| `number` | Integer | 从 1 起的已派发尝试序号 |
| `service` | String | 有界的带 Namespace 的 EdgionBackend 键（`namespace/name`） |
| `provider` | String | 固定提供商枚举 |
| `credential` | String | 非敏感配置凭据别名；绝不是提供商凭据或 Secret 数据 |
| `status` | Integer | 收到时的提供商 HTTP 状态 |
| `failure` | String | 固定类别：`retryable_status`、`ambiguous_transport`、`backend_request_timeout` 或 `incomplete_response`。`incomplete_response` 表示**提供商**提前中断；由网关自身结束的流（超配额，或配额已无法执行）不是提供商故障，不带 `failure`，请改看其 `EdgionStatus` 码 |
| `retry_after_seconds` | Integer | 接受的有界 Retry-After 冷却，单位秒 |
| `ttft_ms` | Integer | 最终成功尝试到达首个非空响应体分片的时间 |
| `usage_source` | String | `reported` 或 `estimated` |
| `input_tokens` | Integer | 该次尝试的上报或有界估算输入 token |
| `output_tokens` | Integer | 该次尝试的上报或有界估算输出 token |
| `estimated_cost_usd` | String | 配置定价时的估算尝试成本 |

账本从不存储提供商 URL、解析后的地址、Secret 值、请求/响应体、提供商错误文本、prompt、completion
或工具参数。对派发后的歧义结果，Redis 可能保守地保留完整的「输入 + 最大输出」预留直至窗口过期。
该预留用于保护限流；它与此处记录的持久观测/有界估算用量不同，也不表示已消耗全部预留 token。

### `stage_logs` 字段

`stage_logs` 是阶段组的数组；每个组包含一个执行阶段的每插件日志。

```json
"stage_logs": [
  {
    "stage": "request_filters",
    "logs": [
      { "name": "BasicAuth", "time_cost": 5, "log": ["Auth success"] },
      { "name": "Cors", "time_cost": 2, "log": ["CORS resp set"] }
    ]
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `stage` | String | 执行阶段。示例：`request_filters`、`upstream_response_filters` |
| `logs[].name` | String | 插件名称 |
| `logs[].time_cost` | Integer | 插件执行时间（**微秒**） |
| `logs[].log` | Array\<String\> | 插件输出的日志行 |
| `logs[].log_full` | Boolean | 仅在插件日志缓冲区溢出并丢弃条目时出现且为 `true` |

常见插件日志行：

| 插件 | 日志行 | 含义 |
|------|--------|------|
| BasicAuth | `Auth success` / `Auth failed` / `Anonymous` | 认证结果 |
| Cors | `CORS resp set` / `Preflight handled` / `Origin rejected` | CORS 处理结果 |
| CSRF | `Token verified` / `Token set` / `Token mismatch` / `No token in header` | CSRF 处理结果 |
| IPRestriction | `Allowed` / `Denied` | IP 过滤结果 |
| Mock | `Mock returned` | 返回了 mock 响应 |

`log_full: true` 表示插件输出的日志数据超过了每插件缓冲区的容量，后续行被丢弃。将其视为需要精简插件日志输出的提示，而非错误。

### `errors` 取值

| 值 | 说明 |
|----|------|
| `RouteNotFound` | 请求无匹配路由 |
| `XffHeaderTooLong` | `X-Forwarded-For` 头超过允许长度 |
| `BackendNotFound` | 后端服务无法解析 |
| `UpstreamConnectError` | 上游连接失败 |

## 扩展日志：AccessLogExtern

access log 出厂时是固定字段集。若要追加路由元数据、请求/响应字段或插件派生值，唯一支持的方式是声明 `AccessLogExtern`。它取代了此前五个互不相关的旧载体（路由注解标签投影、JSON 白名单注解、逗号分隔的 ctx-key 注解、以及 CtxSet 与注解之间的隐式耦合），这些旧机制已全部删除，不保留别名。

### 它是声明，不是插件

`AccessLogExtern` 条目从不作为请求过滤器执行。它位于 `EdgionPlugins.spec.accessLogExtern`——一个专用数组，不属于任何请求/响应插件阶段——仅在写入日志行时才被读取。正因如此，即使响应根本没有经过任何插件，`AccessLogExtern` 字段依然会出现：未匹配路由的 404、421 SNI 不匹配、400 系列、ACME 挑战响应，或被短路的 CORS 预检。

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionPlugins
metadata:
  name: audit-fields
  namespace: default
spec:
  accessLogExtern:
    - key: tenant        # 日志条目 `ext` 块内的字段名
      from: ctx           # 来源类型
      name: tenant_id     # 该来源内的 key
    - key: ua
      from: header
      name: user-agent
```

每个条目仅为一个字段声明一个来源——不存在把多个值组合进一个字段的模板语法。若需要组合值（如 `"{tenant}:{tier}"`），先用 `CtxSet` 插件计算一次，再用 `from: ctx` 记录这个 ctx 变量。在 `AccessLogExtern` 层做组合的方案已被评估并否决：组合后的字符串没有单一的来源归属，逐字段的脱敏无法正确应用于它。

### 来源

| `from` | 读取内容 | 是否脱敏 |
|---|---|---|
| `routeLabel` | 匹配路由（HTTPRoute/GRPCRoute）的 `metadata.labels` 键 | 从不脱敏 |
| `routeAnnotation` | 匹配路由的 `metadata.annotations` 键（`kubectl.kubernetes.io/*` 与 `edgion.io/*` 键会在准入时上报到 `Accepted=False`，并在网关加载该资源时被丢弃——见下方硬黑名单） | 从不脱敏 |
| `header` | 请求头（重复头取第一个值；非 UTF-8 值视为不存在） | 除非白名单，否则脱敏 |
| `query` | 客户端实际发送的 query 字符串，冻结于请求入口——不受后续 rewrite 影响 | 除非白名单，否则脱敏 |
| `cookie` | 请求 cookie（`set-cookie`/响应 cookie 不可达——见下方硬黑名单） | 除非白名单，否则脱敏 |
| `respHeader` | 实际写出的响应头；上游从未响应时（连接失败、超时）不存在 | 除非白名单，否则脱敏 |
| `ctx` | 插件设置的上下文变量（`CtxSet`、DSL `ctx.set`、webhook `KeyGet` 结果等）——读取时机在每个阶段的每个生产者写入之后 | 除非白名单，否则脱敏 |

有一小部分名称在任何脱敏设置下都不可作为来源。命名它们会在 `EdgionPlugins` 资源的 `Accepted=False` 状态条件中被报告；与该报告相互独立，网关在加载该资源时还会丢弃该字段，因此它永远不会真正被投影进日志行：

| 来源 | 被禁止的名称 |
|---|---|
| `header` / `respHeader` | `authorization`、`proxy-authorization`、`cookie`、`set-cookie`、`x-api-key`、`api-key`、`apikey`、`x-auth-token`、`authentication`、`x-amz-security-token`、`www-authenticate`、`proxy-authenticate` |
| `ctx` | `jwt_claims`、`oidc_claims`、`jwe_payload` |
| `routeAnnotation` | `kubectl.kubernetes.io/` 或 `edgion.io/` 前缀下的任何键 |

### 字段落在哪里

所有采集到的字段进入同一个嵌套的 `ext` 对象，仅嵌套一层，值始终渲染为字符串：

```json
{"ts": 1752633600000, "request_info": {"host": "api.example.com", "path": "/users"}, "ext": {"tenant": "acme", "ua": "***"}}
```

若该请求未采集到任何字段，`ext` 会从这一行整体省略。

### 挂载层级

`AccessLogExtern` 最多可在三个层级声明，按以下顺序生效——同名 key 冲突时后声明的层级覆盖先声明的层级：

| 顺序 | 层级 | 声明位置 |
|---|---|---|
| 1 | GatewayClass 全局 | `EdgionGatewayConfig.spec.globalPluginsRef` |
| 2 | Gateway | `Gateway` 上的 `edgion.io/edgion-plugins` 注解 |
| 3 | Route | HTTPRoute / GRPCRoute 上指向某个 `EdgionPlugins` 的 `ExtensionRef` filter |

**全局审计字段应放在第 1 层。** 第 2 层（Gateway 注解）在任何未匹配到路由的响应上都是空的——404、421、400 系列、ACME、gRPC `UNIMPLEMENTED`——因为一个请求本应使用的 Gateway 只有在路由匹配解析*之后*才知道。第 1 层直接从 proxy service 解析，因此在包括这些响应在内的每一条响应上都存在。这是预期行为而非缺陷：在匹配之前就赋值 Gateway 意味着要在一个监听器可能服务的多个 Gateway 之间猜测。

**给全局层的 key 加上有辨识度的前缀**（例如用 `gw_tenant` 而非 `tenant`），因为冲突解决是静默的：如果某个路由层字段恰好与全局字段同名，路由的值会胜出，而日志不会给出任何提示表明一个全局字段被覆盖了。

### 脱敏

`routeLabel` 与 `routeAnnotation` 的值从不脱敏——它们是操作员在路由自身元数据中编写的配置，而非请求数据。其余所有来源默认脱敏，除非该具体 key 在 `EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys` 中被列入白名单（按来源类型分——`header`、`respHeader`、`query`、`cookie`、`ctx`）。白名单 schema 与默认条目见 [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)。

白名单**未命中仍会输出该字段**——是脱敏而非丢弃——因此操作员看到的是 `"ext":{"session":"***"}`，能明确知道该值已解析出来只是未加入白名单，而不是面对一个缺失的字段去猜测：究竟是来源没有值、字段名写错了，还是一个被禁止的名称已在 `Accepted=False` 中上报、并在网关加载该资源时被丢弃了。

脱敏渲染遵循与内置敏感字段相同的全局开关 `access_log_sensitive.mode`（见[网关配置 → access_log_sensitive](../configuration/gateway-config.md#access_log_sensitive)）。**从本版本起 `access_log_sensitive.mode` 默认值为 `replace`**，因此开箱即可、无需任何额外配置：`ext` 字段只要未加入白名单就会渲染为 `***`；而 `path`、`direct_peer_ip`、`remote_ip`、`x-forwarded-for`、`sni`、`request_uri` 这六个内置字段则是**无条件**渲染为 `***`——白名单对这六个内置字段不生效，它们始终经由 `masked::serialize` 脱敏，与 `unmaskedKeys` 无关。

**不存在既保留 `path` 可读、又对 `ext` 中的 cookie 脱敏的配置。** `mode: off` 会让白名单失效（一切都渲染为明文，包括 `ext`）；`mode: wrap` 会把原始值写进网关自己的本地日志文件/stdout，用 `<E>value</E>` 标记以便下游管道在入库前剥离——但原始值仍留在本地磁盘上。如果需要可读的 path，请在这两种权衡中明确二选一；不存在第三种既保留内置字段明文又能脱敏 `ext` 的配置。

### 已知限制（设计如此，非缺陷）

- **`redirect_http` 监听器（纯 HTTP→HTTPS 重定向服务）永不输出 `ext`。** 它使用一套独立的极简日志 schema，根本没有 `ext` 字段——该路径上没有插件链可供声明挂载。
- **gRPC-web/WebSocket 连接建立时产生的第二条 access log 行不带 `ext`**——只有该请求的最终日志行才带。这是正确行为：采集器只针对最终那条日志行运行。
- **`debug-access-log` 插件写入的 `X-Debug-Access-Log` 响应头故意永不携带 `ext`。** 若填充它，将把操作员声明的字段值回传给发起请求的客户端。

### 完整示例

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  accessLogExtern:
    unmaskedKeys:
      header: [user-agent, origin, content-type, content-length, accept, x-request-id]
      ctx: [tenant_id]
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  rules:
    - backendRefs: [{ name: api-service, port: 8080 }]
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: audit-fields
```

```json
{"ts": 1752633600000, "request_info": {"host": "api.example.com", "path": "/users", "status": 200},
 "ext": {"tenant": "acme", "ua": "Mozilla/5.0"}}
```

上方[声明](#它是声明不是插件)的 `audit-fields` 资源所定义的两个字段在此都渲染为明文：`ua`（`from: header, name: user-agent`）被 `unmaskedKeys.header` 加入白名单，`tenant`（`from: ctx, name: tenant_id`）被 `unmaskedKeys.ctx` 加入白名单。

## 特殊字段

### 原始请求 URI

默认情况下日志记录规范化后的 `path`。要同时捕获客户端发送的原始 URI（规范化/重写之前），在顶层 `access_log_options` 段启用 `include_raw_uri`：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  include_raw_uri: true
```

原始 URI 随后出现在 `request_uri` 字段中。调试路径重写或 URL 编码问题时有用，但会增加每行大小，因此仅在排查时启用。

## 常见配置场景

### 1. 快速本地调试

使用默认配置将日志写入本地文件——开发期间查看网关行为的最简方式：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

### 2. 生产环境按日轮转

每天保留一个文件，保留一周历史：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### 3. 排查路径重写问题

调试重写问题时同时捕获规范化路径和客户端原始 URI：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  include_raw_uri: true
```

## 日志示例

### 基础 HTTP 请求

```json
{
  "ts": 1767089146376,
  "request_info": {
    "direct_peer_ip": "192.168.1.20",
    "client-port": 54882,
    "remote_ip": "192.168.1.20",
    "x-trace-id": "eff524a7-10a4-483c-a534-3c50b1199aa0",
    "host": "test.example.com",
    "path": "/health",
    "status": 200
  },
  "match_info": { "rns": "production", "rn": "api-route", "rule_id": 0, "match_id": 0 },
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 200, "ct": 0, "ht": 0, "bt": 0, "et": 1 }
    ]
  }
}
```

### 带重试的请求

```json
{
  "ts": 1767089146377,
  "request_info": {
    "host": "api.example.com",
    "path": "/api/data",
    "status": 200
  },
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 502, "ct": 1, "ht": 100, "et": 100, "err": ["Connection reset"] },
      { "ip": "10.0.0.6", "port": 8080, "status": 200, "ct": 2, "ht": 50, "bt": 50, "et": 52 }
    ]
  }
}
```

### 路由未匹配

```json
{
  "ts": 1767089149015,
  "request_info": {
    "host": "wrong-hostname.example.com",
    "path": "/test",
    "status": 404
  },
  "errors": ["RouteNotFound"],
  "backend_context": null
}
```

### gRPC 请求

```json
{
  "ts": 1767089148470,
  "request_info": {
    "host": "grpc.example.com",
    "path": "/test.TestService/SayHello",
    "status": 200,
    "discover_protocol": "grpc",
    "grpc_service": "test.TestService",
    "grpc_method": "SayHello"
  },
  "backend_context": {
    "name": "grpc-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.21", "port": 8080, "status": 200, "ct": 0, "ht": 1, "bt": 1, "et": 1 }
    ]
  }
}
```

## 使用 jq 分析日志

```bash
# 所有 4xx 响应
cat logs/edgion_access.log | jq 'select(.request_info.status >= 400 and .request_info.status < 500)'

# 按上游首字节时间排序最慢的请求
cat logs/edgion_access.log | jq -r '[.request_info.path, .backend_context.upstreams[0].ht] | @tsv' | sort -k2 -n

# 耗时超过 100 微秒的插件执行
cat logs/edgion_access.log | jq '.stage_logs[]?.logs[]? | select(.time_cost > 100)'

# 端到端追踪一个请求
cat logs/edgion_access.log | jq 'select(.request_info["x-trace-id"] == "eff524a7-10a4-483c-a534-3c50b1199aa0")'
```

重点关注的字段：

- `ht`（首字节时间）：后端首字节时间；值高说明后端延迟高。
- `ct`（连接时间）：值高说明网络或连接池问题。
- `et`（总耗时）：每次尝试的总时间，含重试。
- `stage_logs[].time_cost`：插件开销（微秒）。
- `log_full: true`：插件日志缓冲区溢出。

## 常见问题

### Q1：我启用了 access log，但文件为空。为什么？

**A**：确认 `enabled: true` 已设置，且 `output.localFile.path` 解析到可写位置。相对路径相对于 `work_dir` 解析，请检查网关进程的实际工作目录。同时确认请求确实到达了网关。

### Q2：为什么某些字段从日志行中缺失？

**A**：可选字段为空时省略，而非写入 `null`。例如，无路由匹配时 `match_info` 不存在，`grpc_service` 只出现在 gRPC 请求，`sni` / `tls_id` 只出现在 TLS 连接。这是预期行为，保持日志行紧凑。

### Q3：如何关联多跳请求的日志？

**A**：从客户端发送 `X-Trace-ID` 头。网关将其记录为 `x-trace-id`，可以用单个 `jq` 选择器过滤一个逻辑请求的所有条目。

### Q4：请求被插件拒绝了——看哪里？

**A**：查看 `stage_logs`。拒绝插件的 `log` 数组通常包含原因（例如 BasicAuth 的 `Auth failed` 或 Cors 的 `Origin rejected`）。当插件短路请求时，`backend_context` 通常为 `null`。

### Q5：如何获取某个路由的更详细插件诊断？

**A**：使用 debug-access-log 插件提升每插件的详细程度。见 [debug-access-log](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md)。

## 获取帮助

如遇问题：

1. 检查本文档的常见问题部分
2. 检查网关日志
3. 在 GitHub 提交 issue

## 相关文档

- [监控指标](./metrics.md)
- [debug-access-log 插件](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md)
- [edgion-cli](../edgion-cli.md)
