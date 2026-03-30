---
name: resource-dependency
description: 资源间依赖关系速查：spec 引用、Controller 内 Store 查询、requeue 级联。
---

# 资源间依赖关系

> 详细分析见 [tasks/references/resource-dependency-map.md](../../../../tasks/references/resource-dependency-map.md)

## 依赖关系索引图

```
                        ┌──────────────────────────────────────────────────────────┐
                        │  Secret  ──on_change──► requeue 所有 SecretRefManager 登记方 │
                        └──────────────────┬───────────────────────────────────────┘
                                           │ 被引用（certificateRef / secretRef / 凭据）
                    ┌──────────────────────┼──────────────────────┐
                    ▼                      ▼                       ▼
                Gateway               EdgionTls            EdgionPlugins
           BackendTLSPolicy           EdgionAcme

                        ┌──────────────────────────────────────────────────────────┐
                        │  ConfigMap  ──on_change──► requeue SecretRefManager 登记方 │
                        └──────────────────┬───────────────────────────────────────┘
                                           │ 被引用（frontend CA）
                                           ▼
                                        Gateway

                        ┌──────────────────────────────────────────────────────────┐
                        │  Service  ──on_change──► requeue ServiceRefManager 登记方  │
                        └──────────────────┬───────────────────────────────────────┘
                                           │ 被引用（backendRef / clusterIP）
                    ┌──────────────────────┴──────────────────────┐
                    ▼                                              ▼
          各类 Route（HTTP/GRPC/TCP/TLS/UDP）                   Gateway
         （验证 backend 存在性）                          （推导 status 地址）

                        ┌──────────────────────────────────────────────────────────┐
                        │  ReferenceGrant  ──on_change──► requeue 跨 ns 引用方       │
                        └──────────────────┬───────────────────────────────────────┘
                                           │ 被查询（跨 ns 引用权限校验）
                    ┌──────────────────────┴──────────────────────┐
                    ▼                                              ▼
                Gateway（跨 ns Secret/CA）              各类 Route（跨 ns backendRef）

                        ┌──────────────────────────────────────────────────────────┐
                        │  Namespace  ──label 变更──► requeue Selector Gateway       │
                        └──────────────────┬───────────────────────────────────────┘
                                           │ 被查询（allowedRoutes Selector 策略）
                                           ▼
                                     各类 Route（listener 是否允许该 namespace）

                        ┌──────────────────────────────────────────────────────────┐
                        │  Gateway  ──on_change──► requeue gateway_route_index 中的 Route │
                        └──────────────────┬───────────────────────────────────────┘
                                           │ 被引用（parentRef）
                    ┌──────────────────────┴──────────────────────┐
                    ▼                                              ▼
          各类 Route（验证 listener 匹配）                      EdgionTls
          ──on_change──► requeue 父 Gateway
```

## Store 与写入/读取方

| Store | 写入方 | 读取方 |
|-------|--------|--------|
| **SecretStore** | SecretHandler | Gateway、EdgionTls、EdgionPlugins、BackendTLSPolicy、EdgionAcme |
| **ConfigMapStore** | ConfigMapHandler | Gateway（frontend CA） |
| **NamespaceStore** | K8s Namespace informer | 各类 Route（allowedRoutes Selector） |
| **ReferenceGrantStore** | ReferenceGrantHandler | Gateway、各类 Route（跨 ns 权限） |
| **SecretRefManager** | Gateway/EdgionTls/EdgionPlugins/BackendTLSPolicy/EdgionAcme（parse 时注册） | SecretHandler/ConfigMapHandler（on_change 时查依赖方） |
| **ServiceRefManager** | 各类 Route（parse 时注册 backend） | ServiceHandler（on_change 时查依赖方） |
| **CrossNsRefManager** | Gateway/各类 Route（parse 时注册跨 ns 引用） | ReferenceGrantHandler（on_change 时查依赖方） |
| **GatewayRouteIndex** | 各类 Route/EdgionTls（on_change 时写入） | GatewayHandler（on_change 时查关联 Route） |

## 无外部依赖的资源

EdgionGatewayConfig、EdgionStreamPlugins、LinkSys、PluginMetaData、Endpoints/EndpointSlice、Namespace
