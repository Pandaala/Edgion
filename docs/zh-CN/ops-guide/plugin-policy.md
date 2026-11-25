# GatewayClass 插件执行策略

插件执行策略配置在 `EdgionGatewayConfig.spec.pluginPolicy`，通过
`GatewayClass.spec.parametersRef` 绑定。相同的 EdgionPlugins 或
EdgionStreamPlugins 可以在不同 GatewayClass 下采用不同策略。修改 EGC 后，
策略通过正常配置同步动态生效，无需重启 Controller、Gateway 或现有监听器。

## 配置与绑定

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: public-policy
spec:
  pluginPolicy:
    deny:
      - name: http/Wasm
      - name: http/Dsl
        action: block
        blockStatus: 451
      - name: stream/ConnectionRateLimit
        action: bypass
    defaultAction: allow
    deniedAction: block
    blockStatus: 503
---
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: public-policy
```

使用 `gatewayClassName: public` 的 Gateway 受此策略管理。多个 Class 可以绑定
同一个 EGC；未被引用的 EGC 不影响插件执行。Kubernetes 环境应先安装更新后的
结构化 CRD，否则旧 schema 可能裁剪 `pluginPolicy` 字段。

Kubernetes Controller 接管所有 `spec.controllerName` 与部署身份匹配的 GatewayClass。
每个 Gateway 通过自己引用的 class 的 `parametersRef` 选择策略，因此同一部署的
多个 class 可以使用不同策略。多个 class 应使用不同的物理监听端口。

插件名称使用区分大小写的 `<family>/<Type>`：`http` 覆盖四个 HTTP 插件阶段，
`stream` 覆盖连接阶段，`tls-route` 覆盖 TLSRoute 阶段。不同 family 的同名插件
分别管理，不支持通配符。未知名称、重复项和超出 400～599 的状态码均无效。

`deny` 优先于 `allow`。设置 `allow` 后，未列出的名称均被限制；`allow: []`
表示限制全部插件。没有 allow 列表时，未匹配的名称遵循 `defaultAction`，
默认为 `allow`。受限名称遵循 `deniedAction`，默认为 `block`；deny 条目可以
覆盖动作和 HTTP 状态码。采用或继承 `bypass` 的条目不能同时配置 `blockStatus`。

完整字段见[配置参考](../../../skills/02-features/02-config/02-edgion-gateway-config.md#specpluginpolicy)。
Controller 的旧 `validation.plugin_policy` 字段已删除，配置解析会拒绝它，
没有兼容别名。

## 执行与热更新

**Block** 无条件阻断插件链，优先于条目条件和无效配置门禁。HTTP 返回配置的
状态码及通用响应体 `{"message":"plugin disabled by policy"}`，不向客户端暴露
插件名称。即使受限插件位于响应阶段，也会在请求准入时阻断。同一资源按请求、
响应头、响应体、异步响应阶段及列表顺序选取首个 Block 的状态码。
Stream 和 TLSRoute 直接拒绝准入，不使用 HTTP 状态码。

**Bypass** 跳过受限实现并继续执行插件链。绕过认证插件也会移除相应认证检查。
策略只控制命名插件的执行，不移除二进制中的代码，也不限制原生 Gateway API
filter 或资源级原生 WAF 配置。

已启用的已知 HTTP 插件遇到配置解码错误时，针对该插件名的 Block 仍返回配置的状态码。
允许或绕过该插件时，插件链仍会因配置错误返回 HTTP 500。

Gateway 只为实际服务、引用且允许该插件的作用域构造实例；同一源配置代的有状态
实例在允许的作用域之间共享。策略变更会释放当前作用域不再需要的实例，已准入
请求可以持有旧实例直到结束。

无效 DSL/WAF 更新仍可按既有规则保留兼容的上一份有效配置，但当前策略同时约束
新配置和保留候选，新 Block 不会被旧配置覆盖。最新程序仍无效时，放宽策略不会
重新构造旧配置中已排除的条目，可能返回配置错误 500，直到程序修复。跨命名空间
镜像及其他不可保留的校验错误仍按原规则拒绝替换。

## 生效边界

- 未设置 parametersRef，或引用的 EGC 存在但没有 `pluginPolicy`，表示允许全部。
- 被引用的 EGC 缺失、删除或策略语义无效时，HTTP 返回 503，新 Stream 准入被拒绝。
  如果资源在 API Server 或反序列化阶段已被拒绝，则没有发布新配置，旧的已接受输入仍保留。
- 每个 HTTP 请求保存实际逻辑 Gateway 的作用域，后续响应、body 和 backend 插件
  使用同一视图。新 keep-alive/H2 请求读取新策略；已准入请求、连接和 UDP 会话不会强制撤销。
  Backend ExtensionRef 仅在实际选中的 backend 首次使用时，按请求已捕获的作用域执行；
  未选中候选的限制不会在选择 backend 之前拒绝请求。
- 多个 Gateway 共享物理监听器时，只有连接阶段的有效插件链等价才执行一次；冲突时
  拒绝新连接。配置化 Early RequestId 在逻辑归属不明确时延迟到路由匹配之后执行。
- 配置在进程和资源类型之间异步传播，不承诺全局同时生效或跨资源原子事务。

## 观测与核验

Controller 在 EGC 上报告策略配置错误。共享插件资源只保留源配置诊断，不再发布
全局 `PluginPolicyRestricted` 条件。Controller 状态不代表所有 Gateway 已应用策略。

HTTP 使用既有 `PluginPolicyBlock` / `PluginPolicyBypass` 访问日志条目，以及
`edgion_gateway_plugin_policy_blocked_total` 和
`edgion_gateway_plugin_policy_bypassed_total` 指标；Stream 使用既有流日志。
应向各受影响 Class 发起新请求或新连接验证行为。管理接口的 store-stats 包含
发布器持有数量和 key 清单，可用于检查配置清理后的残留。
