# 负载均衡策略配置指南

> **🔌 Edgion 扩展**
> 
> 通过 `ExtensionRef` 配置负载均衡算法是 Edgion 的扩展功能。

本文档说明如何通过 HTTPRoute 的 `extensionRef` 配置可选的负载均衡算法。

## 概述

Edgion 默认使用 RoundRobin 负载均衡算法。您可以通过以下方式为特定服务启用额外的算法：
- **Ketama**: 一致性哈希算法
- **FnvHash**: FNV哈希算法
- **LeastConnection**: 最少连接算法

## 配置方式

直接在 HTTPRoute 的 filter 中通过 `extensionRef.name` 指定负载均衡算法。算法配置会自动应用到该 rule 的所有 backendRefs。

### 基本示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - api.example.com
  rules:
    - filters:
        # 直接在 extensionRef.name 中指定算法
        - type: ExtensionRef
          extensionRef:
            name: ketama  # 单个算法
      backendRefs:
        - name: my-service
          port: 8080
```

## 支持的算法

| 算法名称 | 别名 | 说明 |
|---------|------|------|
| `ketama` | `consistent-hash` | 一致性哈希，适用于缓存场景 |
| `fnvhash` | `fnv-hash` | FNV哈希算法 |
| `leastconn` | `least-connection`, `leastconnection`, `least_connection` | 最少连接算法 |

## 工作原理

1. **策略提取**: 当 HTTPRoute 被创建或更新时，Edgion 会扫描所有 `ExtensionRef` 类型的 filter
2. **算法解析**: 从 `extensionRef.name` 中解析算法名称
3. **服务映射**: 将算法应用到该 rule 的所有 backendRefs 指定的服务
4. **策略存储**: 将服务到算法的映射存储到全局 PolicyStore 中
5. **按需加载**: 当 EndpointSlice 创建时，会根据服务的策略按需初始化对应的负载均衡器

## 生命周期管理

- **引用计数**: PolicyStore 会跟踪每个服务被多少个 HTTPRoute 引用
- **自动清理**: 当最后一个引用该服务的 HTTPRoute 被删除时，对应的策略会自动清理
- **更新处理**: HTTPRoute 更新时会自动刷新相关的策略配置

### 手动删除策略

除了自动清理外，您也可以手动删除特定 HTTPRoute 的负载均衡策略：

```rust
use edgion::core::lb::optional_lb::get_global_policy_store;

// 获取全局策略存储
let store = get_global_policy_store();

// 根据资源键删除策略
store.delete_lb_policies_by_resource_key("default/my-route");
```

**注意事项：**
- 此操作会删除指定 HTTPRoute 对所有服务的策略引用
- 如果某个服务只被该 HTTPRoute 引用，则该服务的策略会被完全清除
- 如果某个服务还被其他 HTTPRoute 引用，则该服务的策略仍然保留

## 示例场景

### 场景 1: 缓存服务使用一致性哈希

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cache-route
  namespace: default
spec:
  parentRefs:
    - name: gateway
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            name: ketama  # 一致性哈希
      backendRefs:
        - name: redis-cache
          port: 6379
```

### 场景 2: API 服务使用最少连接

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-routes
  namespace: prod
spec:
  parentRefs:
    - name: api-gateway
  hostnames:
    - api.mycompany.com
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /users
      filters:
        - type: ExtensionRef
          extensionRef:
            name: leastconn  # 最少连接
      backendRefs:
        - name: user-api
          port: 8080
    
    - matches:
        - path:
            type: PathPrefix
            value: /orders
      filters:
        - type: ExtensionRef
          extensionRef:
            name: leastconn
      backendRefs:
        - name: order-api
          port: 8080
```

## 注意事项

1. **算法格式**: `extensionRef.name` 中的算法名称不区分大小写
2. **单一配置**: 每个 rule 只能配置一个负载均衡算法
3. **作用范围**: 算法配置会应用到同一个 rule 中所有的 backendRefs
4. **默认行为**: 未配置策略的服务继续使用默认的 RoundRobin 算法

## 故障排查

查看日志中的相关信息：

```bash
# 查看策略提取日志
kubectl logs <edgion-pod> | grep "LB policy"

# 查看策略应用日志
kubectl logs <edgion-pod> | grep "Added LB policies"

# 查看策略清理日志
kubectl logs <edgion-pod> | grep "Removed LB policies"
```

常见问题：

- **策略未生效**: 检查 `extensionRef.name` 中的算法名称是否正确
- **算法名称错误**: 使用支持的算法名称或别名（参见上面的算法表格）

