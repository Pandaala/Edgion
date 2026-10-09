# ExtProc 插件

> **🔌 Edgion 扩展**
>
> ExtProc 通过 gRPC 双向流把请求/响应的各阶段交给外部处理服务。它由 `EdgionPlugins` CRD 提供，并**与 Envoy `envoy.service.ext_proc.v3` 线协议兼容**，因此现有的 ext_proc 服务端实现/各语言 SDK 可直接复用。它是一个独立插件，和 [ForwardAuth](./forward-auth.md) 平级——两者可以共存。

## ExtProc 解决什么问题？

有些 header 逻辑过于动态、无法用静态配置表达：从外部策略引擎改写 header、从旁路服务为请求做增强、或让另一个团队用他们自己的语言掌管请求/响应裁决。ExtProc 把这类决策委托给一个外部 gRPC 服务：

- 网关把某个请求/响应阶段以 `ProcessingRequest` 流给处理服务。
- 处理服务回 `ProcessingResponse`，可以**改 header**（add / set / remove），或返回 **`ImmediateResponse`** 直接短路请求（例如返 `403`）。
- 注意区分两个「gRPC」：**控制通道**（网关 ↔ 处理服务）固定是 gRPC；**被代理的业务流量**是普通 HTTP。ExtProc 是把 HTTP 流量的每个阶段「用 gRPC 包装」后喂给处理服务。

### 支持范围

| 能力 | 支持 | 相位 |
|---|:---:|---|
| 读/改**请求** header | ✅ | `RequestFilter` |
| 请求阶段短路（`ImmediateResponse`，如 403） | ✅ | `RequestFilter` |
| 读/改**响应** header | ✅ | 请求条目持有的内部 response companion |
| 响应 header 尚未提交时短路（`ImmediateResponse`） | ✅ | 内部 response companion |
| 读/替换**请求 body**（整体缓冲） | ✅ | `requestBodyMode: BUFFERED` |
| 逐块检查/改写**请求 body**（流式） | ✅ | `requestBodyMode: STREAMED` |
| 逐窗口读/替换/清空**响应 body** | ✅ | `responseBodyMode: STREAMED`，保持主响应流式 |
| 读/改真实的**响应 trailer** | ✅ | `responseTrailerMode: SEND`；支持 H2 和可表达的 H1.1 trailer |
| 由处理服务缩减后续相位 | ✅ | 显式允许时使用 header verdict 的 `mode_override` |

请求 `BUFFERED_PARTIAL`、响应 `BUFFERED` 和响应 `BUFFERED_PARTIAL` 会**被校验拒绝**。响应 `STREAMED` 当前处理的是有界 transport window；它不把 Pingora transport chunk 当作 SSE event，也不宣称具备 semantic SSE window。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ext-proc-basic
  namespace: edgion-default
spec:
  # 请求 header 处理
  requestPlugins:
    - type: ExtProc
      config:
        grpcService:
          target: "ext-proc.svc.cluster.local:9000"
          timeoutMs: 200
        processingMode:
          requestHeaderMode: SEND
          responseHeaderMode: SEND
          responseBodyMode: STREAMED
          responseTrailerMode: SKIP
        disableImmediateResponse: false
        allowModeOverride: false
        allowedOverrideModes: []
        failureModeAllow: false   # 处理服务不可达时 fail-close
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: ext-proc-route
  namespace: edgion-default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
  hostnames:
    - "ext-proc.example.com"
  rules:
    - matches:
        - path: { type: PathPrefix, value: "/" }
      filters:
        - type: ExtensionRef
          extensionRef: { group: edgion.io, kind: EdgionPlugins, name: ext-proc-basic }
      backendRefs:
        - name: test-http
          namespace: edgion-test
          port: 30001
```

> ExtProc 只能配置在 `requestPlugins`。一个匹配的条目是唯一 transaction owner；它创建不可 Clone 的请求级 logical `Process` stream，并由内部 response companion 继续使用同一条 stream。不能在 `upstreamResponsePlugins` 再配置第二个 ExtProc 条目来拼接事务。

## 配置参数

| 参数 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `grpcService.target` | String | —（必填） | 处理服务的 gRPC 端点，纯 `host:port`（不是 URL）。IPv6 必须加方括号：`[::1]:9000`。 |
| `grpcService.timeoutMs` | Integer | `200` | 每条消息的超时（毫秒），范围 `1..=60000`；覆盖发送、首次 lazy open 和等待 verdict。没有总 stream deadline，合法的长时间 SSE/下载可长期保持空闲。 |
| `grpcService.tls` | Object | 未启用 | 处理服务连接的 TLS 配置：`enabled`、`verify`、`validation.caCertificateRefs`、`clientCertificateRef`（mTLS）。缺省/`enabled: false` = 明文 h2c（集群内默认）；未设置的子字段继承全局 `outboundTls` 默认。 |
| `processingMode.requestHeaderMode` | Enum | `SEND` | `SEND` 把请求 header 发给处理服务，`SKIP` 跳过。 |
| `processingMode.responseHeaderMode` | Enum | `SKIP` | 同一请求条目在最终响应 header 阶段的 `SEND` / `SKIP`。 |
| `processingMode.requestBodyMode` | Enum | `NONE` | `NONE`、`SKIP`、`BUFFERED` 或 `STREAMED`。`BUFFERED` 先缓冲完整请求 body 再送处理服务；`STREAMED` 逐块随流交换（与 header 相位共用一条请求级 `Process` 流，逐块改写、上游改用 chunked 传输、请求不可重试）。`BUFFERED_PARTIAL` 被校验拒绝。 |
| `processingMode.responseBodyMode` | Enum | `NONE` | `NONE`、`SKIP` 或 `STREAMED`。`STREAMED` 对每个有界 transport window 严格 lockstep 交换；响应仍逐块释放，绝不聚合完整 upstream response。 |
| `processingMode.responseTrailerMode` | Enum | `SKIP` | `SEND` 只为真实上游 trailer 发送相位，绝不伪造空消息；`SKIP` 不处理 trailer。 |
| `disableImmediateResponse` | Boolean | `false` | 为 `true` 时忽略任意相位的 `ImmediateResponse`，不应用其中的 mutation，停用本次请求的 ExtProc，并让当前 HTTP 交换原样继续。 |
| `allowModeOverride` | Boolean | `false` | 允许处理服务在请求/响应 header verdict 中，从 `allowedOverrideModes` 精确选择一种模式。override 只能关闭后续能力，不能增加能力。 |
| `allowedOverrideModes` | Processing mode 列表 | `[]` | `mode_override` 的精确、归一化 allow-list；启用时必须有 1–8 项。每项必须是基础 `processingMode` 的缩减，归一化后重复的条目会被拒绝；`allowModeOverride` 为 false 时列表必须为空。 |
| `failureModeAllow` | Boolean | `false` | 处理服务不可达、超时、或返回不可用 verdict 时：`false`（fail-close）以 `500` 短路请求；`true`（fail-open）跳过处理继续放行。 |

## 行为

### Header mutation

处理服务返回 `HeaderMutation`（`set_headers` + `remove_headers`）。每个 `set_headers` 条目带 `append_action`：

| `append_action` | 效果 |
|---|---|
| `APPEND_IF_EXISTS_OR_ADD`（默认） | 追加一个值（多值 header）。 |
| `ADD_IF_ABSENT` | 仅当该 header 当前不存在时设置。 |
| `OVERWRITE_IF_EXISTS_OR_ADD` | 设置（覆盖）。 |
| `OVERWRITE_IF_EXISTS` | 仅当该 header 当前存在时设置。 |

网关把该阶段的 header 发给处理服务，包含只读的 HTTP/2 伪 header（请求侧 `:method`/`:path`/`:authority`，响应侧 `:status`），方便处理服务据此路由。多值 header（如 `Set-Cookie`）会被保真传递，不会被折叠成单值。

发送的 header 值使用 protobuf `raw_value`，保留 HTTP 允许的非 UTF-8 字节。处理服务返回 mutation 时，非空 `raw_value` 优先于 `value`；两者均为空时，仅在 `keep_empty_value: true` 时应用空值。请使用 `raw_value` 返回原始字节。请求、响应及 `ImmediateResponse` 的 header 值都经过 HTTP header 校验；含 CR、LF、NUL 等非法字节的写操作会被跳过，并记录不包含原始值的访问日志诊断。

条件动作按 header 名称是否存在判断，即使现有值不是 UTF-8，也视为存在。同名值保留追加顺序；不同名称之间不保证原始报文顺序。Edgion 按列表顺序执行 `set_headers`，随后执行 `remove_headers`。这些动作和空值规则是 Edgion 的行为约定；采用 Envoy 的 `raw_value` 字节表示不意味着两者的所有 mutation 细节完全一致。

### 受保护的 header（安全）

处理服务**不能**改动：

- **伪 header**（`:method`、`:path`、`:status` 等）——防止篡改路由/方法。
- **逐跳 / framing header**——`Content-Length`、`Transfer-Encoding`、`Connection`、`Keep-Alive`、`Upgrade`、`TE`、`Trailer`、`Proxy-Connection`。这些请求侧限制保持不变。响应侧共享策略还拒绝 `Proxy-Authenticate` 和 `Proxy-Authorization`，trailer 保留额外校验。分帧及连接字段由代理管理。

任何对受保护 header 的改动都会被丢弃并记入访问日志。

### ImmediateResponse（短路）

若处理服务在下游响应尚未提交时返回 `ImmediateResponse`，网关停止回源并返回其中的 status 与 body。请求 header 和响应 header 相位中的合法自定义 header mutation 都会进入新建的 local reply；受保护 header 仍会被拒绝。

对于已被规范识别为 gRPC 的请求，显式存在且位于 0–16 的 `grpc_status` 会产生 trailers-only gRPC 回复：HTTP 200、处理服务精确指定的 `grpc-status`，以及由 Immediate body 编码得到的可选 `grpc-message`。字段 presence 会保留，因此状态 0 不会被当成缺失。大于 16 的值属于畸形 verdict，按 `failureModeAllow` 处理，绝不截断或映射。普通 HTTP 会忽略 `grpc_status`，继续使用声明的 HTTP status/body。

设置 `disableImmediateResponse: true` 后，完整的 Immediate 消息会在校验前被忽略：status、body 和 header mutation 均不应用，不再发送后续 ExtProc 相位，后端/当前响应原样继续。抑制不是依赖失败，不会触发 `failureModeAllow`。响应已提交后收到启用的 Immediate 消息不能创建第二个 local reply，只能按提交后的失败策略处理。

### 动态模式缩减

启用 `allowModeOverride` 后，处理服务可以把 Envoy 的 `mode_override` 附在匹配的请求 header 或响应 header verdict 上。Edgion 会归一化完整模式，并且只在它与 `allowedOverrideModes` 中某一项精确相等时接受。

Allow-list 只允许缩减：

- 请求 body 可以保持基础 `BUFFERED`/`STREAMED` acquisition class，或缩减为 `NONE`；不能新启用 body 访问，也不能切换 acquisition class；
- 响应 body 可以保持 `STREAMED`，或缩减为 `NONE`；
- 响应 header 和响应 trailer 可以保持原配置，或从 `SEND` 缩减为 `SKIP`；
- 请求 header 相位已经消费，归一化时忽略；配置中的 body `SKIP` 会归一化为线协议 `NONE`。

例如，下面的基础模式只允许处理服务选择列表中的请求 body 抑制；它不能获得 buffered 访问，也不能关闭其他相位：

```yaml
processingMode:
  requestHeaderMode: SEND
  requestBodyMode: STREAMED
  responseHeaderMode: SEND
  responseBodyMode: STREAMED
  responseTrailerMode: SKIP
allowModeOverride: true
allowedOverrideModes:
  - requestHeaderMode: SKIP       # 已消费；归一化为 SKIP
    requestBodyMode: NONE         # 允许 STREAMED -> NONE 缩减
    responseHeaderMode: SEND
    responseBodyMode: STREAMED
    responseTrailerMode: SKIP
```

功能未启用时收到的 override，以及结构合法但不在 allow-list 或扩张能力的 override，会被忽略。启用后，畸形枚举/结构属于 bad verdict。独立 override 与 body/trailer 相位的 override 不受支持。

### 流式请求的边界

网关分别累计原始输入字节和处理链的最终输出字节，并检查当前请求固定的
`EdgionGatewayConfig.spec.requestBody` 物理上限。输入在处理前检查，输出在所有
流式处理器完成后、RequestMirror 和上游转发前检查。恰好达到上限允许通过；
超限或计数溢出固定终止为 HTTP 413，即使 `failureModeAllow: true` 或处理器已
fail-open 停用。中间块仍受处理器传输消息大小限制。

安装过流式处理器的请求携带真实 request trailers 时，网关返回 HTTP 400，
不会伪造干净的 EOS verdict。拒绝会取消未完成的处理器和镜像交换；代理已选择
响应时不会再写第二个响应。上游 HTTP/1.1 使用 chunked framing；HTTP/2 使用
DATA，不发送 `Content-Length` 或 `Transfer-Encoding`。

多个流式处理器按执行顺序串行等待 verdict，RequestMirror 只接收最终字节。
完整 body 捕获/修改与流式处理不能混用，两个执行顺序都由跨插件作用域的运行时
保护拒绝：捕获后安装流式处理器遵循处理器失败策略，安装后捕获或修改则走现有
body 访问失败策略，不读取或发布字节。普通请求的 trailers 丢弃行为保持不变。

### 响应 trailer

普通 Edgion 构建已通过固定的 Edgion Pingora revision 提供该能力。

使用 `responseTrailerMode: SEND` 时，真实的上游 trailer map 会作为同一条 Process 流最后一条 `response_trailers` 请求发送。上游没有 trailer 时，Edgion 会干净地关闭流，不伪造空 trailer 相位。`SKIP` 会让真实 trailer 不经过 ExtProc。

Trailer mutation 保留重复 raw value，并采用原子提交：网关先校验克隆后的 map，因此 fail-open 不会暴露半套 mutation。输入和 mutation 后的完整结果都限制为最多 256 个字段、raw name/value 总计 64 KiB。伪 header、`host`、`proxy-authenticate`、`proxy-authorization`，以及代理管理的 framing/逐跳字段（`connection`、`keep-alive`、`proxy-connection`、`transfer-encoding`、`content-length`、`te`、`upgrade`、`trailer`）均禁止。gRPC 响应最终必须恰好保留一个 0–16 的合法 `grpc-status`；只有非 gRPC 响应允许移除它。

H2 和 custom transport 可以引入合法的新 trailer 名。对于 H1.1 下游，新名字必须已出现在提交的 `Trailer` 声明中；已观察到的名字仍可修改或移除。H1.0 无法表达响应 trailer：fail-open 会在 transport 边界丢弃它们，fail-close 会终止连接。Trailer 处理不会缓冲或聚合响应 body。

### 失败处理（`failureModeAllow`）

`failureModeAllow` 不仅在处理服务不可达/超时时生效，处理服务返回**畸形或错相位 verdict** 时同样生效——一个有 bug 的处理服务不能静默绕过已配置的 fail-close。

- `false`（fail-close，鉴权类用法推荐）：以不含依赖、目标或插件身份细节的通用 `500` 短路。
- `true`（fail-open，非关键增强场景）：跳过处理，继续回源。

## 限制

- **响应 body 不缓冲**：仅支持 `STREAMED` transport window；不创建 response-side `BodyStorage`，不聚合主响应。
- **不支持 `BUFFERED_PARTIAL`**。
- **不支持 `FULL_DUPLEX_STREAMED`**。
- **不处理请求 trailer**：`responseTrailerMode` 不会启用请求 trailer 相位；普通代理请求 trailer 尚未接入 ExtProc。
- **同一 request transaction 只有一条 logical stream**：阶段严格按 request headers → request body windows → final response headers → response body windows → EOS/cancellation 排序。共享的是 target-scoped tonic Channel；logical `Process` stream 不共享、不可 Clone、不可在发送过 phase 后透明重连或重放。response-only 模式会 lazy open。
- **窗口上限**：输入 window 与 replacement output 均采用 inclusive 1 MiB hard cap 和 checked arithmetic；超过上限按失败策略处理。可能改变 body 长度时移除 `Content-Length`。
- **SSE 边界**：当前接入点接收 bounded transport chunks，不是 semantic SSE events。未来由 canonical SSE raw-byte hold/release adapter 在上游喂入；本插件不会复制第二套 decoder。未修改时直接释放原字节，不 parse 后重序列化。
- **`STREAMED` 语义**：逐块改写可能改变总长度，上游请求改用 chunked 传输（移除 `Content-Length`）；选中流式插件后请求即不可重试；body 相位 verdict 中的 header mutation 会被视为 bad verdict，并按照 `failureModeAllow` 拒绝（上游 header 已发出）；mid-body 的 `ImmediateResponse` 会应用其 header mutation（保护头拒绝），gRPC 路由映射为 `grpc-status` trailers；`CONTINUE_AND_REPLACE`（header 相位或 mid-body）一律按 bad verdict 处理；Upgrade/CONNECT 请求与 HTTP/1.1 以下客户端无法流式检查，按失败策略处理。
- **请求 body 被放弃**：若主上游提前完成响应并停止接收仍在上传的请求 body，网关会取消请求侧 `Process` 流，不会发送 `end_of_stream=true`。处理服务已收到的数据只是前缀，不能作为完整 body 结算。
- **响应提交后的错误**：只能安全终止当前 stream 并记录聚合日志，不能改写成新的 HTTP 错误响应。
- **未包含较新的 Envoy 控制面**：动态 timeout override、request drain、observability mode、send-body-without-waiting 和 streamed ImmediateResponse 不在当前裁剪协议内。

## 兼容性

proto 是 `envoy.service.ext_proc.v3` 的裁剪但线协议兼容子集（字段号一致）。现成的 ext_proc 服务端（任意语言 SDK）均可互通，只要停留在上表支持的相位内。

## 常见问题

### Q：ExtProc 和 ForwardAuth 能挂在同一条 route 上吗？

可以。它们是独立插件。把两者都列进 `requestPlugins`（按你想要的执行顺序）——例如 ForwardAuth 先鉴权，再由 ExtProc 增强 header。

### Q：处理服务返回了一个 header，但客户端/后端看不到？

检查相位：请求 header mutation 发往**上游**（后端），响应 header mutation 发往**客户端**。确认唯一条目位于 `requestPlugins`，并把对应模式设为 `SEND`。如果该 header 是 `Content-Length`/`Connection` 等受保护字段，它会被有意拒绝。

### Q：Controller 拒绝了我的 EdgionPlugins 资源？

常见原因：请求配置了 `BUFFERED_PARTIAL`，响应配置了 `BUFFERED`/`BUFFERED_PARTIAL`，`allowedOverrideModes` 中存在扩张/重复/非法条目，`allowModeOverride` 为 false 时列表非空，ExtProc 放在了非 `requestPlugins` 阶段，`grpcService.target` 写成 URL/缺端口，或 `timeoutMs` 超出范围。

### Q：处理服务挂了会怎样？

由 `failureModeAllow` 决定：fail-close（`false`）返回 `500`；fail-open（`true`）原样放行未处理的请求。

## 相关链接

- [ForwardAuth 插件](./forward-auth.md)
- [Response Rewrite 插件](./response-rewrite.md)
- [过滤器总览](../README.md)

## 响应头策略

响应、trailer 和本地回复修改使用共享策略，包括禁止 `Proxy-Authenticate` 和 `Proxy-Authorization`。请求头限制不变；trailer 专属校验及 gRPC 状态要求仍然适用。

[响应头策略](../gateway-api/response-header-modifier.md#response-header-policy).
