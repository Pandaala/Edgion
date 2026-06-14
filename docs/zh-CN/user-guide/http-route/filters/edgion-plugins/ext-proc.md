# ExtProc 插件

> **🔌 Edgion 扩展**
>
> ExtProc 通过 gRPC 双向流把请求/响应的各阶段交给外部处理服务。它由 `EdgionPlugins` CRD 提供，并**与 Envoy `envoy.service.ext_proc.v3` 线协议兼容**，因此现有的 ext_proc 服务端实现/各语言 SDK 可直接复用。它是一个独立插件，和 [ForwardAuth](./forward-auth.md) 平级——两者可以共存。

## ExtProc 解决什么问题？

有些 header 逻辑过于动态、无法用静态配置表达：从外部策略引擎改写 header、从旁路服务为请求做增强、或让另一个团队用他们自己的语言掌管请求/响应裁决。ExtProc 把这类决策委托给一个外部 gRPC 服务：

- 网关把某个请求/响应阶段以 `ProcessingRequest` 流给处理服务。
- 处理服务回 `ProcessingResponse`，可以**改 header**（add / set / remove），或返回 **`ImmediateResponse`** 直接短路请求（例如返 `403`）。
- 注意区分两个「gRPC」：**控制通道**（网关 ↔ 处理服务）固定是 gRPC；**被代理的业务流量**是普通 HTTP。ExtProc 是把 HTTP 流量的每个阶段「用 gRPC 包装」后喂给处理服务。

### 本期范围

本期 ExtProc **仅处理 header**：

| 能力 | 支持 | 相位 |
|---|:---:|---|
| 读/改**请求** header | ✅ | `RequestFilter` |
| 请求阶段短路（`ImmediateResponse`，如 403） | ✅ | `RequestFilter` |
| 读/改**响应** header | ✅ | 异步 `UpstreamResponse` |
| 响应阶段短路（`ImmediateResponse`） | ✅ | 异步 `UpstreamResponse` |
| 读/改**请求 body** | ❌ | —（无 request-body 钩子） |
| 读/改**响应 body** | ❌ | —（response-body 钩子只读） |

**不支持 body 处理**。把 body 模式配成 `NONE`/`SKIP` 以外的值会**被校验拒绝**（Controller 将资源标记为无效）——绝不会静默忽略。

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
        failureModeAllow: false   # 处理服务不可达时 fail-close
  # 响应 header 处理（独立条目，异步阶段）
  upstreamResponsePlugins:
    - type: ExtProc
      config:
        grpcService:
          target: "ext-proc.svc.cluster.local:9000"
          timeoutMs: 200
        processingMode:
          responseHeaderMode: SEND
        failureModeAllow: false
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

> 请求阶段和响应阶段是**两个独立条目**（`requestPlugins` 与 `upstreamResponsePlugins`）。只需要哪个阶段就配哪个。每个条目只读自己阶段的模式（`requestPlugins` 条目读 `requestHeaderMode`，`upstreamResponsePlugins` 条目读 `responseHeaderMode`）。

## 配置参数

| 参数 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `grpcService.target` | String | —（必填） | 处理服务的 gRPC 端点，纯 `host:port`（不是 URL）。IPv6 必须加方括号：`[::1]:9000`。 |
| `grpcService.timeoutMs` | Integer | `200` | 单消息超时（同时约束连接与等待 verdict）。范围 `1..=60000`。 |
| `grpcService.tls` | Boolean | `false` | 是否经 TLS 连接（服务端认证，系统/webpki 根证书）。`false` = 明文 h2c（集群内默认）。 |
| `processingMode.requestHeaderMode` | Enum | `SEND` | `SEND` 把请求 header 发给处理服务，`SKIP` 跳过。仅 `requestPlugins` 条目读取。 |
| `processingMode.responseHeaderMode` | Enum | `SKIP` | 响应 header 的 `SEND` / `SKIP`。仅 `upstreamResponsePlugins` 条目读取——在那里要设成 `SEND`。 |
| `processingMode.requestBodyMode` | Enum | `NONE` | 必须为 `NONE` 或 `SKIP`。`STREAMED` / `BUFFERED` / `BUFFERED_PARTIAL` 会**被校验拒绝**（不支持 body 处理）。 |
| `processingMode.responseBodyMode` | Enum | `NONE` | 限制同 `requestBodyMode`。 |
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

### 受保护的 header（安全）

处理服务**不能**改动：

- **伪 header**（`:method`、`:path`、`:status` 等）——防止篡改路由/方法。
- **逐跳 / framing header**——`Content-Length`、`Transfer-Encoding`、`Connection`、`Keep-Alive`、`Upgrade`、`TE`、`Trailer`、`Proxy-Connection`。这些由代理自身管理；让外部服务设置它们会导致请求/响应走私。（对齐 Envoy 默认 `disallow_system_headers` 的做法。）

任何对受保护 header 的改动都会被丢弃并记入访问日志。

### ImmediateResponse（短路）

若处理服务返回 `ImmediateResponse`，网关停止回源，直接把该状态码 + body 返回给客户端。**请求**阶段会把该响应的自定义 header mutation 应用到回包；**响应**阶段只应用状态码和 body（运行时会重建回包，故响应阶段 `ImmediateResponse` 的自定义 header 不保留——gRPC route 上仅保留状态码）。

### 失败处理（`failureModeAllow`）

`failureModeAllow` 不仅在处理服务不可达/超时时生效，处理服务返回**畸形或错相位 verdict** 时同样生效——一个有 bug 的处理服务不能静默绕过已配置的 fail-close。

- `false`（fail-close，鉴权类用法推荐）：以 `500 ext_proc unavailable` 短路。
- `true`（fail-open，非关键增强场景）：跳过处理，继续回源。

## 限制

- 本期**仅 header**（见「本期范围」）。不处理请求/响应 body。
- **无跨阶段关联**：每个阶段各自开一条短生命周期 `Process` 流（MVP 简化），因此处理服务无法把同一事务请求阶段的状态带到响应阶段。连接池 / 单事务单流是后续优化。
- **响应阶段 `ImmediateResponse`**：自定义 header 不保留（仅状态码 + body；gRPC route 仅状态码）。

## 兼容性

proto 是 `envoy.service.ext_proc.v3` 的裁剪但线协议兼容子集（字段号一致）。现成的 ext_proc 服务端（任意语言 SDK）均可互通，只要停留在本期支持的「仅 header」相位内。

## 常见问题

### Q：ExtProc 和 ForwardAuth 能挂在同一条 route 上吗？

可以。它们是独立插件。把两者都列进 `requestPlugins`（按你想要的执行顺序）——例如 ForwardAuth 先鉴权，再由 ExtProc 增强 header。

### Q：处理服务返回了一个 header，但客户端/后端看不到？

检查相位：请求 header mutation 发往**上游**（后端），响应 header mutation 发往**客户端**。还要确认条目在对应的列表里（`requestPlugins` vs `upstreamResponsePlugins`）且对应模式设为 `SEND`。如果该 header 是 `Content-Length`/`Connection` 之类，那是被有意拒绝的（见「受保护的 header」）。

### Q：Controller 拒绝了我的 EdgionPlugins 资源？

常见原因：`requestBodyMode`/`responseBodyMode` 配了 `NONE`/`SKIP` 以外的值；`grpcService.target` 写成了 URL 或缺端口；`timeoutMs` 为 `0` 或超过 `60000`。

### Q：处理服务挂了会怎样？

由 `failureModeAllow` 决定：fail-close（`false`）返回 `500`；fail-open（`true`）原样放行未处理的请求。

## 相关链接

- [ForwardAuth 插件](./forward-auth.md)
- [Response Rewrite 插件](./response-rewrite.md)
- [过滤器总览](../README.md)
