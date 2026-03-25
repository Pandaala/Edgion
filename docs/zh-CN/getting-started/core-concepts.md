# 核心概念

这页用于快速建立 Edgion 的最小心智模型，不展开实现细节。

## 1. Controller 和 Gateway 是分离的

Edgion 不是把所有事情都塞在一个进程里。

- **Controller**
  负责读取配置、校验资源、更新状态，并把可用配置同步给 Gateway。
- **Gateway**
  负责真正接收流量、做路由匹配、执行插件、TLS 处理和转发。

如果你更关心部署和使用，这个区别知道到这里就够了；如果想看实现，去读：

- [开发指南 / 架构概览](../dev-guide/architecture-overview.md)

## 2. GatewayClass 决定“谁来管理 Gateway”

`GatewayClass` 是控制面入口。

它最重要的一条字段是：

```yaml
controllerName: edgion.io/gateway-controller
```

这表示这个 `GatewayClass` 由 Edgion 控制器接管。

## 3. Gateway 定义流量入口

`Gateway` 决定：

- 开哪些 listener
- 每个 listener 用什么协议
- 监听哪个端口
- 是否需要 TLS
- 允许哪些 route 绑定

可以把它理解成“流量入口声明”。

## 4. Route 资源定义怎么匹配和怎么转发

不同协议用不同 route：

- `HTTPRoute`
- `GRPCRoute`
- `TCPRoute`
- `UDPRoute`
- `TLSRoute`

它们共同回答的问题是：

- 什么请求会命中
- 命中后做哪些处理
- 最后转发到哪里

## 5. Edgion 在 Gateway API 之上加了扩展能力

除了标准 Gateway API 资源，Edgion 还提供一些扩展：

- `EdgionPlugins`
- `EdgionStreamPlugins`
- `EdgionTls`
- `EdgionGatewayConfig`

这些扩展通常用于：

- HTTP/TCP/TLS 插件
- 更细粒度的 TLS 能力
- Gateway 级高级配置

如果你看到文档里的 `🔌 Edgion 扩展` 标记，说明它不是标准 Gateway API 的一部分。

## 6. 学习顺序建议

如果你是第一次接触 Edgion，推荐顺序是：

1. [第一个 Gateway](./first-gateway.md)
2. [运维指南 / Gateway 总览](../ops-guide/gateway/overview.md)
3. [用户指南 / HTTPRoute 总览](../user-guide/http-route/overview.md)
4. 根据场景继续看 TLS、filters、plugins 或后端配置

如果你已经在排查实现或准备扩展功能，再切到：

- [开发指南](../dev-guide/README.md)
