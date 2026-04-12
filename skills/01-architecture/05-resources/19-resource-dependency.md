---
name: resource-dependency
description: 资源间依赖关系速查：spec 引用、Controller 内 Store 查询、requeue 级联。
---

# 资源间依赖关系

> 详细分析见 [tasks/references/resource-dependency-map.md](../../../../tasks/references/resource-dependency-map.md)

## 依赖关系索引图

```mermaid
flowchart LR
    subgraph Secret依赖
        Secret -- "on_change requeue\nSecretRefManager 登记方" --> Gateway
        Secret -- "certificateRef\nsecretRef / 凭据" --> EdgionTls
        Secret --> EdgionPlugins
        Secret --> BackendTLSPolicy
        Secret --> EdgionAcme
    end

    subgraph ConfigMap依赖
        ConfigMap -- "on_change requeue\nSecretRefManager 登记方" --> Gateway2[Gateway]
        ConfigMap -- "frontend CA\nbackend CA" --> BackendTLSPolicy2[BackendTLSPolicy]
    end

    subgraph Service依赖
        Service -- "on_change requeue\nServiceRefManager 登记方" --> Routes["各类 Route\n（HTTP/GRPC/TCP/TLS/UDP）"]
        Service -- "backendRef\nclusterIP" --> Gateway3["Gateway\n（推导 status 地址）"]
    end

    subgraph ReferenceGrant依赖
        ReferenceGrant -- "on_change requeue\n跨 ns 引用方" --> Gateway4["Gateway\n（跨 ns Secret/CA）"]
        ReferenceGrant -- "跨 ns 权限校验" --> Routes2["各类 Route\n（跨 ns backendRef）"]
    end

    subgraph Namespace依赖
        Namespace -- "label 变更 requeue\nSelector Gateway" --> Routes3["各类 Route\n（listener 是否允许该 namespace）"]
    end

    subgraph Gateway依赖
        GW[Gateway] -- "on_change requeue\ngateway_route_index" --> Routes4["各类 Route\n（验证 listener 匹配）"]
        GW -- "parentRef" --> EdgionTls2[EdgionTls]
        Routes4 -- "on_change requeue" --> GW
    end
```

## Store 与写入/读取方

| Store | 写入方 | 读取方 |
|-------|--------|--------|
| **SecretStore** | SecretHandler | Gateway、EdgionTls、EdgionPlugins、BackendTLSPolicy、EdgionAcme |
| **ConfigMapStore** | ConfigMapHandler | Gateway（frontend CA）、BackendTLSPolicy（backend CA） |
| **NamespaceStore** | K8s Namespace informer | 各类 Route（allowedRoutes Selector） |
| **ReferenceGrantStore** | ReferenceGrantHandler | Gateway、各类 Route（跨 ns 权限） |
| **SecretRefManager** | Gateway/EdgionTls/EdgionPlugins/BackendTLSPolicy/EdgionAcme（parse 时注册） | SecretHandler/ConfigMapHandler���on_change 时查依赖方） |
| **ServiceRefManager** | ��类 Route（parse 时注册 backend） | ServiceHandler（on_change 时查依赖方）、BackendTLSPolicy（resolve_ancestor_gateways 时读取） |
| **CrossNsRefManager** | Gateway/各类 Route（parse 时注册跨 ns 引用） | ReferenceGrantHandler（on_change 时查依赖方） |
| **GatewayRouteIndex** | 各类 Route/EdgionTls（on_change 时写入） | GatewayHandler（on_change 时查关联 Route）、BackendTLSPolicy（resolve_ancestor_gateways 时读取） |
| **BackendTLSPolicyTargetManager** | BackendTLSPolicy（parse 时注册 targetRef） | BackendTLSPolicy（update_status 时冲突检测） |

## 无外部依赖的资源

EdgionGatewayConfig、EdgionStreamPlugins、LinkSys、PluginMetaData、Endpoints/EndpointSlice、Namespace
