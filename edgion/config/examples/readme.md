# Edgion Gateway 配置关系概览

本文总结 `GatewayClass`、`Gateway`、`HTTPRoute` 在 Edgion 中的对应关系与同步逻辑，便于在 Kubernetes 中编排网关资源时快速定位关键字段。

## 1. 基础对象与参数引用

| 资源类型       | 关键字段 / 作用                                                                 | 在 Edgion 中的映射                                               |
|----------------|--------------------------------------------------------------------------------|-------------------------------------------------------------------|
| `GatewayClass` | `metadata.name` 决定网关类型；`spec.controllerName` 指定控制器；`spec.parametersRef` 可以引用全局配置 (`EdgionGatewayConfig`) | 在 `ConfigCenter` 里以 **GatewayClassKey**（即 `metadata.name`）建缓存；`ConfigHub` 同步后也以相同 key 管理 |
| `EdgionGatewayConfig` | 通过 `parametersRef` 关联到 `GatewayClass`，提供监听、负载、限流、观测等默认设置 | 在 `ConfigCenter` 中以固定 key（示例中为 `test-gateway-class`）存储，并传播到 `ConfigHub` |

## 2. Gateway 与 GatewayClass 的绑定

- `Gateway` 使用 `spec.gatewayClassName` 指定所依赖的 `GatewayClass`。
- 在 `ConfigCenter::event_add` 中，`Gateway` 会按照 `gatewayClassName` 写入以该名称为 key 的缓存。
- 因此：
  - 一个 `GatewayClass` 可以对应多个 `Gateway`；
  - 删除或更新 `GatewayClass` 时，不会自动清除 `Gateway`，但在业务侧可根据 key 做关联操作。

## 3. HTTPRoute 的匹配流程

`HTTPRoute` 通过 `spec.parentRefs` 指定将规则挂载在哪些 `Gateway` 上：

1. `ConfigCenter` 解析 `parentRefs` 列表，取第一个引用（示例代码中未展开 namespace 逻辑，默认相同命名空间）。
2. `parentRef.name` 用来匹配目标 `Gateway`，`parentRef.sectionName`（若提供）需与 `Gateway.spec.listeners[].name` 对应，标识具体监听器。
3. 以 `parentRef.name` 作为缓存 key，将 `HTTPRoute` 存到该网关的路由列表中。
4. 当 `Gateway` 接收到路由缓存更新时，可根据自身名称（即前一步的 key）获取所有绑定的 `HTTPRoute`；如果设置了 `sectionName`，业务逻辑可进一步筛选监听器。

> **注意**：若一个 `HTTPRoute` 需要同时作用于多个 `Gateway`，需要在 `parentRefs` 中列出多个目标；当前实现会逐一写入每个 `Gateway` 的路由缓存。

## 4. 同步路径一览

```
GatewayClass(metadata.name) ─┐
                             │
                             ├─> ConfigCenter.gateway_classes[key]
                             │
EdgionGatewayConfig ────────┘ (通过参数引用)

Gateway(spec.gatewayClassName) ──> ConfigCenter.gateways[gatewayClassName]

HTTPRoute(spec.parentRefs[0].name) ──> ConfigCenter.routes[gatewayName]
```

`ConfigHub` 会以相同的 key 结构（GatewayClassKey、Gateway 名称）持久化同步结果，供 gRPC Client 侧快速查询。

## 5. 使用建议

1. **先创建 `EdgionGatewayConfig` 与 `GatewayClass`**，确保默认配置与控制器均已就绪。
2. **再创建 `Gateway` 并指定 `gatewayClassName`**，确认映射关系正确。
3. **最后发布 `HTTPRoute`**，`parentRefs` 中至少包含一个目标 `Gateway`，否则不会被路由缓存收纳。
4. 如需多租户或多环境隔离，可通过不同的 `GatewayClass` / `Gateway` 组合实现。

完整示例可参考：

- `config/examples/edgion_gateway_config__public.yaml`
- `config/examples/gateway.yaml`
- `config/examples/httproute.yaml`（若有）
