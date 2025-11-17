# Edgion Gateway 配置关系概览

本文总结 `GatewayClass`、`Gateway`、`HTTPRoute` 在 Edgion 中的对应关系与同步逻辑，便于在 Kubernetes 中编排网关资源时快速定位关键字段。

## 1. 基础对象与参数引用

| 资源类型       | 关键字段 / 作用                                                                 | 在 Edgion 中的映射                                               |
|----------------|--------------------------------------------------------------------------------|-------------------------------------------------------------------|
| `GatewayClass` | `metadata.name` 决定网关类型；`spec.controllerName` 指定控制器；`spec.parametersRef` 可以引用全局配置 (`EdgionGatewayConfig`) | 在 `ConfigCenter` 里以 **GatewayClassKey**（即 `metadata.name`）建缓存；`ConfigHub` 同步后也以相同 key 管理 |
| `EdgionGatewayConfig` | 通过 `parametersRef` 关联到 `GatewayClass`，提供监听、负载、限流、观测等默认设置 | 在 `ConfigCenter` 中以固定 key（示例中为 `test-gateway-class`）存储，并传播到 `ConfigHub` |

## 2. Gateway 与 GatewayClass 的绑定

- `Gateway` 使用 `spec.gatewayClassName` 指定所依赖的 `GatewayClass`。
- 在 `ConfigCenter::apply_change` 处理中，当收到 `ResourceChange::EventAdd` 时，`Gateway` 会按照 `gatewayClassName` 写入以该名称为 key 的缓存。
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

- `config/examples/_public-gateway_EdgionGatewayConfig.yaml` - EdgionGatewayConfig（cluster-scoped）
- `config/examples/_edgion-gateway-public_GatewayClass.yaml` - GatewayClass（cluster-scoped）
- `config/examples/default_gateway1_Gateway.yaml` - Gateway（namespace-scoped）
- `config/examples/test1_public-route1_HTTPRoute.yaml` - HTTPRoute（namespace-scoped）

## 6. 文件命名规范

为了确保文件系统配置加载器能正确识别和处理配置文件，**所有配置文件必须遵循严格的命名规范**：

### 6.1 命名格式

- **有 namespace 的资源**：`{namespace}_{name}_{kind}.yaml`
  - 示例：`default_gateway1_Gateway.yaml`
  - 示例：`test1_public-route1_HTTPRoute.yaml`（注意：name 中的连字符 `-` 在文件名中保持不变）

- **Cluster-scoped 资源**（无 namespace）：`_{name}_{kind}.yaml`
  - 示例：`_edgion-gateway-public_GatewayClass.yaml`
  - 示例：`_public-gateway_EdgionGatewayConfig.yaml`

### 6.2 命名规则说明

1. **文件名必须与文件内容完全匹配**：
   - `namespace` 必须与 YAML 文件中的 `metadata.namespace` 完全一致
   - `name` 必须与 YAML 文件中的 `metadata.name` 完全一致（包括连字符和下划线）
   - `kind` 必须与 YAML 文件中的 `kind` 完全一致

2. **文件扩展名**：只支持 `.yaml` 或 `.yml`

3. **Cluster-scoped 资源**：如果资源没有 `namespace` 字段（如 `GatewayClass`），文件名必须以 `_` 开头

### 6.3 文件内容要求

1. **必须是有效的 YAML 格式**
2. **必须包含完整的资源定义**：
   - `kind` 字段（必需）
   - `metadata.name` 字段（必需）
   - `metadata.namespace` 字段（对于 namespace-scoped 资源必需）

3. **文件名与内容必须一致**：如果文件名与文件内容中的 metadata 不匹配，文件将被跳过并记录错误日志

### 6.4 错误处理

如果文件名不符合规范，系统会：
- 记录 `ERROR` 级别的日志
- 在日志中明确提示应该使用的文件名格式
- 跳过该文件，不进行任何处理

### 6.5 示例

**正确的文件命名**：
```
# 文件：default_gateway1_Gateway.yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gateway1
  namespace: default
spec:
  ...
```

```
# 文件：_edgion-gateway-public_GatewayClass.yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion-gateway-public
spec:
  ...
```

**错误的文件命名**（将被跳过）：
- `gateway1.yaml` - 缺少 namespace 和 kind
- `default_gateway1.yaml` - 缺少 kind
- `gateway1_Gateway.yaml` - 缺少 namespace（应该是 `default_gateway1_Gateway.yaml`）
