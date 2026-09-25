# Stream Plugin 开发指南

本文档面向需要扩展 `EdgionStreamPlugins` 的贡献者，解释 stream plugin 在 Edgion 中的定位、两阶段运行模型，以及人类开发时最容易漏掉的接线点。

> 面向 AI / Agent 的主 workflow 入口现在是 [https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/02-gateway/13-stream-plugin-dev.md](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/02-gateway/13-stream-plugin-dev.md)。
> 本文档保留给人看的背景说明、实现边界和手工核对清单。

## 新增插件类型无需修改 CRD

连接阶段和 TLSRoute 阶段的插件名称在 CRD 中都是开放字符串。实现并注册新的 Rust 枚举变体时，无需增加 YAML 白名单条目。
不支持的名称会产生 `Accepted=False`，并拒绝经过该资源的流量；禁用条目或配置策略 bypass 都不能绕过此检查。
已知配置仍保持强类型校验。内部 `Unsupported` 占位不进入受支持插件列表，并丢弃未知配置内容。
新增公共条目字段仍可能需要更新 CRD schema。

## 这类插件解决什么问题

`EdgionStreamPlugins` 是 Edgion 的 L4 扩展机制，用来在 HTTP 解析之前、接纳 UDP 会话时或 TLSRoute 匹配之后执行额外处理。

和 HTTP 插件相比，它的关注点不同：

- 作用层级更早，可以在握手前直接拒绝连接
- 主要处理 IP、监听端口、SNI、mTLS 这类连接上下文
- 失败动作通常不是返回 HTTP 响应，而是直接拒绝或终止连接

当前最成熟的实现是 IP 限制插件，代码入口在：

- `edgion-gateway/src/plugins/stream/ip_restriction/`

## 当前两阶段模型

Edgion 现在把 stream plugin 分成两个阶段：

### Stage 1：连接上下文运行时

特点：

- 对基于 TCP 的监听器，运行在 TLS 握手和 HTTP 解析之前；对 UDPRoute，则运行在新会话首个数据报转发之前
- 只有客户端 IP、监听端口、远端地址等信息
- 适合做尽早拒绝的连接级判断

Gateway 级附件使用 Pingora 的监听器 `ConnectionFilter`，但 UDP 监听器是独立的 Tokio 任务，不使用该桥接。UDPRoute 附件会在 UDP 代理接纳会话时执行同一个 `spec.plugins` 运行时。

典型场景：

- IP allow/deny
- 对特定监听端口的连接门禁

### Stage 2：TlsRoute

特点：

- 运行在 `TLSRoute` 匹配之后：Terminate 在握手完成后执行，Passthrough 在转发端到端握手之前执行
- 可以看到 SNI、匹配到的 Route；协商后的 TLS 身份和 mTLS 状态仅在 Terminate 下可用
- 适合依赖 TLS 上下文的策略判断

典型场景：

- 按 SNI 或 mTLS 状态做限制
- 只在某条 `TLSRoute` 命中后执行策略

#### TLSRoute 阶段契约

Terminate 和 Passthrough 模式下，TLSRoute 都只执行 `spec.tlsRoutePlugins`，不会回退执行 `spec.plugins`。

- 未声明 `tlsRoutePlugins` 或将其设为 null，同时 `plugins` 非空时，拒绝连接。即使 Stage-1 条目全部禁用或 Bypass 也如此：这是阶段绑定错误。
- 显式 `tlsRoutePlugins: []`、Stage-2 条目全部禁用或 Bypass，表示不执行路由阶段策略；两个字段都缺失或为空也允许连接。启用的 Stage-2 Block 条目仍然拒绝连接。
- 引用的资源不存在时仍然拒绝连接。
- TLSRoute 的 IP 限制应从 `plugins` 移至 `tlsRoutePlugins`。GeoIP、ConnectionRateLimit 应通过 Gateway 监听器绑定 `plugins`，它们不属于 Stage-2。共享资源可以同时声明两个字段，各绑定阶段只执行自己的字段。
- 热更新影响新连接，不会重新校验已建立的连接。
- TLS 访问日志记录 `tls route plugin allowed` / `tls route plugin denied`，或资源缺失、阶段绑定错误；连接路径不产生 `tracing` 或 `(compat)` 事件。

如果资源同时包含连接阶段策略，而路由阶段确实需要无策略放行，请显式声明 `tlsRoutePlugins: []`；需要执行 IP 限制时不能以空列表替代策略。

## 当前架构里的关键链路

```mermaid
graph TD
    A["EdgionStreamPlugins YAML"] --> B["Rust resource type"]
    B --> C["Serializable stage configuration"]
    C --> D["Controller ProcessorHandler"]
    D --> E["Config sync to Gateway"]
    E --> F["StreamPluginStore source adapter"]
    P["EGC policy and GatewayClass bindings"] --> Q["Canonical scoped publication"]
    F --> Q
    G["Gateway / TCPRoute / TLSRoute / UDPRoute attachments"] --> Q
    Q --> H["Captured Stage 1 or Stage 2 admission view"]
```

最容易漏的不是“写 trait”，而是中间这几段：

- Gateway 的作用域发布是否为对应阶段建立需求并构造 runtime
- Gateway 是否真的缓存并热更新了 `EdgionStreamPlugins`
- `Gateway` / `TCPRoute` / `TLSRoute` / `UDPRoute` 的注解引用是否真正接到运行时

## 和资源定义相关的代码位置

如果你要新增或扩展 stream plugin，通常先看这些文件：

- `edgion-resources/src/resources/edgion_stream_plugins/mod.rs`
- `edgion-resources/src/resources/edgion_stream_plugins/stream_plugins.rs`
- `edgion-resources/src/resources/edgion_stream_plugins/tls_route_plugins.rs`
- `edgion-resources/src/resource/meta/impls.rs`

这里分别负责：

- CRD 结构、status 和可序列化的阶段配置条目
- Stage 1 插件枚举
- Stage 2 插件枚举
- 资源元数据的 schema 层行为

一个重要事实是：

- Gateway 的 `StreamPluginStore` 将源配置交给统一发布器；`StreamPluginGeneration` 仅构造有需求且被策略允许的阶段条目，并在 class 视图之间共享实例。Controller 传输解析后的源配置并报告自身校验状态，不写入执行策略标记

所以只改 Controller handler 并不够：还需要在 Gateway 的构造逻辑中接入插件，资源同步后才有对应的可执行 runtime。

## 和 Gateway 运行时相关的代码位置

Gateway 侧最关键的是这几块：

- `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs`
- `edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs`
- `edgion-gateway/src/plugins/stream/stream_plugin_store.rs`
- `edgion-gateway/src/runtime/server/listener_builder.rs`
- `edgion-gateway/src/routes/tcp/conf_handler_impl.rs`
- `edgion-gateway/src/routes/tls/proxy.rs`
- `edgion-gateway/src/routes/udp/route_unit.rs`
- `edgion-gateway/src/routes/udp/edgion_udp.rs`

理解方式可以简单记成：

- trait / runtime：定义“插件怎么执行”
- store：定义“Gateway 怎么热更新拿到最新资源”
- listener / route：定义“哪条监听器或路由会调用这个资源”

## 注解绑定规则

当前主键是：

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "policy-name"
```

它目前用于以下位置：

- `Gateway`
- `TCPRoute`
- `TLSRoute`
- `UDPRoute`

这不是一个纯文档约定，而是当前实现真正会读取的键。
仓库里历史上出现过 `edgion.io/stream-plugins` 这种旧写法，新改动不应该继续沿用。

在 `Gateway` 上，该值必须是不带命名空间的资源名，并在 Gateway 自身命名空间内解析。
包含 `/` 的值会被 Controller 拒绝，Gateway 还会以 fail-close 方式进行防御。
`TCPRoute`、`TLSRoute` 和 `UDPRoute` 保留各自的 `name` 或 `namespace/name` 约定。
Gateway 级附件不覆盖 UDP 监听器；要控制 UDP 会话接入，应把资源直接附加到 `UDPRoute`。该路由检查只在新的来源地址会话建立时执行一次，不会逐数据报执行，并且只运行 `spec.plugins`。
如果需要集中维护数据并让各命名空间自行拥有插件策略，可为每个命名空间保留本地
`EdgionStreamPlugins`，并通过现有授权机制引用共享 `EdgionConfigData`。

## 开发时建议的顺序

1. 先决定插件属于 Stage 1、Stage 2，还是两个阶段都要支持。
2. 决定配置结构是否可以复用已有配置，而不是复制一份近似类型。
3. 把新类型接入 `EdgionStreamPlugin` 或 `TlsRouteStreamPlugin`。
4. 在对应 runtime 中补插件对象构造逻辑。
5. 确认作用域发布在准入之前构造被策略允许的对应阶段 runtime。
6. 确认引用该资源的 `Gateway` / `TCPRoute` / `TLSRoute` / `UDPRoute` 真正能取到 store key，并使用正确的生命周期。
7. 最后再补测试和样例 YAML。

如果你是让 AI 来做，直接从 skill 入口开始更稳：

- [https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/02-gateway/13-stream-plugin-dev.md](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/02-gateway/13-stream-plugin-dev.md)

## 测试与验证

至少建议覆盖三类验证：

### 1. 资源层

- 资源能被 controller 接收并写出正确 status
- Gateway 一起发布当前 class 策略和有需求的阶段 runtime

### 2. 引用层

- `Gateway` 注解只接受同命名空间的 `name`；`TCPRoute` / `TLSRoute` / `UDPRoute` 注解仍可解析 `name` 或 `namespace/name`
- 删除资源后，store 和后续准入不再解析该资源；已经建立的 UDP 会话仍保持固定状态，不会立即撤销

### 3. 运行层

- 命中插件时行为正确
- 未命中时不会误伤其他连接
- 热更新会影响后续连接和 UDP 会话准入，store 不残留旧 generation；已经建立的 UDP 会话不会重新鉴权

现成样例和集成测试配置主要在：

- `edgion-tests/integration/conf/Gateway/StreamPlugins/`
- `edgion-tests/integration/conf/TCPRoute/StreamPlugins/`
- `edgion-tests/integration/conf/TLSRoute/StreamPlugins/`
- `edgion-tests/integration/conf/UDPRoute/StreamPlugins/`

## 人工审查清单

- 这个插件到底应该运行在 Stage 1 还是 Stage 2
- enum、runtime、module export 是否都接齐
- 注解键是否仍然使用 `edgion.io/edgion-stream-plugins`
- 是否复用了已有配置结构
- 资源更新 / 删除后 Gateway store 是否会影响后续准入，且没有错误假设已建立的 UDP 会话会被撤销
- 测试是否覆盖了“新增、更新、删除”三个阶段

## 相关文档

- [Annotations 指南](./annotations-guide.md)
- [AI 协作与 Skills 使用指南](./ai-agent-collaboration.md)
- [TCPRoute Stream Plugins 用户文档](../user-guide/tcp-route/stream-plugins.md)
- [知识来源映射与维护规则](./knowledge-source-map.md)
