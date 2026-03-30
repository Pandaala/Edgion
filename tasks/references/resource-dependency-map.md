# 资源间依赖关系图

> Controller 侧所有资源的跨资源引用关系梳理。
> 包括：spec 字段引用、parse 阶段查询、on_change/on_delete 级联 requeue。

---

## 依赖关系总览

```
                    ┌─────────────────────────────────────────────────────┐
                    │                   Secret                             │
                    │  on_change → requeue 所有 SecretRefManager 登记方    │
                    └──────┬──────────────────────────────────────────────┘
                           │ 被引用（certificateRef / secretRef）
              ┌────────────┼────────────────────────────────┐
              ▼            ▼                                 ▼
          Gateway      EdgionTls                    EdgionPlugins / EdgionAcme
          BackendTLSPolicy                          （凭据类 secret）

                    ┌─────────────────────────────────────────────────────┐
                    │                  ConfigMap                           │
                    │  on_change → requeue SecretRefManager 登记方（若有） │
                    └──────┬──────────────────────────────────────────────┘
                           │ 被引用（ca_certificate_refs 中 ConfigMap 类型）
                           ▼
                        Gateway（frontend CA）

                    ┌─────────────────────────────────────────────────────┐
                    │                  Service                             │
                    │  on_change → requeue ServiceRefManager 登记方        │
                    └──────┬──────────────────────────────────────────────┘
                           │ 被引用（backendRef）
              ┌────────────┼────────────────────────────────┐
              ▼            ▼                                 ▼
         HTTPRoute    GRPCRoute          TCPRoute / TLSRoute / UDPRoute
                    （地址查询）
                           ▼
                        Gateway（lookup_service_cluster_ip，同名 Service 取 clusterIP）
                        → 通过 RawStore.get_json_by_key("ns/name") O(1) 查询

                    ┌─────────────────────────────────────────────────────┐
                    │                 ReferenceGrant                       │
                    │  on_change → dispatcher → CrossNsRefManager          │
                    │           → requeue 跨 ns 引用方                     │
                    └──────┬──────────────────────────────────────────────┘
                           │ 被查询（check_reference_allowed）
              ┌────────────┼────────────────────────────────┐
              ▼            ▼                                 ▼
          Gateway      HTTPRoute / GRPCRoute        TCPRoute / TLSRoute / UDPRoute
         （跨 ns Secret/CA）  （跨 ns backendRef）

                    ┌─────────────────────────────────────────────────────┐
                    │                  Namespace                           │
                    │  NamespaceStore（label 变更 → requeue Selector GW）  │
                    └──────┬──────────────────────────────────────────────┘
                           │ 被查询（allowedRoutes.namespaces Selector）
                           ▼
                    Route 的 listener_allows_route_namespace

                    ┌─────────────────────────────────────────────────────┐
                    │                  Gateway                             │
                    │  on_change → requeue gateway_route_index 中的 Route  │
                    └──────┬──────────────────────────────────────────────┘
                           │ 被引用（parentRef）
              ┌────────────┼────────────────────────────────┐
              ▼            ▼                                 ▼
         HTTPRoute    GRPCRoute    TCPRoute / TLSRoute / UDPRoute / EdgionTls
         （lookup_gateway 查 listener 匹配）
```

---

## 各资源详细依赖

### Gateway

| 引用方向 | 目标资源 | 查询方式 | 用途 |
|---------|---------|---------|------|
| spec 引用 | Secret | `get_secret`（SecretStore） | listener TLS 证书、CA |
| spec 引用 | ConfigMap | `get_configmap`（ConfigMapStore） | frontend CA（ConfigMap 类型） |
| spec 引用 | Service（同名） | `PROCESSOR_REGISTRY.get("Service")` → `get_json_by_key("ns/name")`（RawStore，O(1)） | 推导 Gateway status 地址（clusterIP） |
| 权限校验 | ReferenceGrant | `get_global_reference_grant_store()` | 跨 ns 引用 Secret/CA 时校验权限 |
| requeue 发出 | 各类 Route / EdgionTls | `gateway_route_index` → `ctx.requeue` | listener/hostname/port 变化时级联 |

**被以下资源 requeue**：Route `on_change`/`on_delete`（通过 `requeue_parent_gateways`）

---

### HTTPRoute / GRPCRoute

| 引用方向 | 目标资源 | 查询方式 | 用途 |
|---------|---------|---------|------|
| spec 引用 | Gateway（parentRef） | `PROCESSOR_REGISTRY.get("Gateway")` → `get_json_by_key("ns/name")`（RawStore，O(1)，不受 gatewayClassName filter 限制） | 验证 listener 匹配、Accepted condition |
| spec 引用 | Service（backendRef） | `PROCESSOR_REGISTRY.get("Service")` → `contains_key` | 验证 backend 存在性、ResolvedRefs |
| 权限校验 | ReferenceGrant | `get_global_reference_grant_store()` | 跨 ns backendRef 权限 |
| namespace 策略 | Namespace | `get_namespace_store()` | allowedRoutes Selector 策略 |
| requeue 发出 | Gateway | `requeue_parent_gateways` | 自身变更通知父 Gateway 重算 |

---

### TCPRoute / TLSRoute / UDPRoute

HTTPRoute / GRPCRoute 的 L4 版本，引用关系相同，区别：
- TLSRoute 有 `hostnames`，TCP/UDP 无
- TLSRoute preparse 阶段提前写入 `gateway_route_index`

---

### EdgionTls

| 引用方向 | 目标资源 | 查询方式 | 用途 |
|---------|---------|---------|------|
| spec 引用 | Gateway（parentRef） | `lookup_gateway` | 验证 listener 匹配（需 HTTPS/TLS listener） |
| spec 引用 | Secret（secretRef） | `get_secret`（SecretStore） | server 证书 |
| spec 引用 | Secret（ca_secret_ref） | `get_secret`（SecretStore） | 客户端 CA |
| namespace 策略 | Namespace | `get_namespace_store()` | allowedRoutes Selector 策略 |

---

### EdgionPlugins

| 引用方向 | 目标资源 | 查询方式 | 用途 |
|---------|---------|---------|------|
| spec 引用 | Secret（多处） | `get_secret`（SecretStore） | BasicAuth/JWT/OIDC/JWE/证书等凭据 |

**被 Secret `on_change` requeue**（通过 SecretRefManager）

---

### BackendTLSPolicy

| 引用方向 | 目标资源 | 查询方式 | 用途 |
|---------|---------|---------|------|
| spec 引用 | Secret（ca_certificate_refs） | `get_secret`（SecretStore） | CA 证书 |
| spec 引用 | Secret（client_certificate_secret_ref） | `get_secret`（SecretStore） | 客户端证书 |
| spec 引用 | Service（targetRefs） | 未做 registry 查询，仅语义引用 | 目标 backend |

---

### EdgionAcme

| 引用方向 | 目标资源 | 查询方式 | 用途 |
|---------|---------|---------|------|
| spec 引用 | Secret（dns01.credential_ref） | `get_secret`（SecretStore） | DNS-01 凭证 |

---

### Secret

- **无** spec 引用其他资源
- `on_change`/`on_delete`：通过 **SecretRefManager** requeue 所有依赖方（Gateway、EdgionTls、EdgionPlugins、BackendTLSPolicy、EdgionAcme 等）

---

### Service

- **无** spec 引用其他资源
- `on_change`/`on_delete`：通过 **ServiceRefManager** requeue 所有以该 Service 为 backend 的 Route

---

### ConfigMap

- **无** spec 引用其他资源
- `on_change`/`on_delete`：通过 **SecretRefManager**（共用键格式）requeue 依赖方
- 注：Gateway 对 CA ConfigMap 在 parse 中未向 SecretRefManager 注册，当前级联可能为空

---

### ReferenceGrant

- **无** spec 引用其他资源（自身描述跨 ns 权限规则）
- `on_change`/`on_delete`：写 **ReferenceGrantStore** + dispatch 事件 → **CrossNsRevalidationListener** → requeue 跨 ns 引用方（Route、Gateway 等）

---

### GatewayClass

- spec 引用 `parametersRef` → EdgionGatewayConfig（仅字符串校验，无 registry 查询）
- 无 requeue 级联

---

### 无外部依赖的资源

以下资源 parse 阶段不查询其他资源，也不发出 requeue：

| 资源 | 说明 |
|------|------|
| EdgionGatewayConfig | 独立配置，无引用 |
| EdgionStreamPlugins | 独立配置，无引用 |
| LinkSys | 独立配置，无引用 |
| PluginMetaData | 独立配置，无引用 |
| Endpoints / EndpointSlice | 数据透传，无引用 |
| Namespace | 只写入 NamespaceStore，被 Route 查询 |

---

## 全局 Store 汇总

| Store | 写入方 | 读取方 |
|-------|--------|--------|
| **SecretStore** | SecretHandler（`replace_all` / `update`） | Gateway、EdgionTls、EdgionPlugins、BackendTLSPolicy、EdgionAcme |
| **ConfigMapStore** | ConfigMapHandler | Gateway（frontend CA） |
| **NamespaceStore** | K8s Namespace informer | Route（`listener_allows_route_namespace`） |
| **ReferenceGrantStore** | ReferenceGrantHandler | Gateway、各类 Route（`check_reference_allowed`） |
| **SecretRefManager** | Gateway、EdgionTls、EdgionPlugins、BackendTLSPolicy、EdgionAcme（parse 时注册） | SecretHandler / ConfigMapHandler（on_change 时查依赖方） |
| **ServiceRefManager** | 各类 Route（`register_service_backend_refs`） | ServiceHandler（on_change 时查依赖方） |
| **CrossNsRefManager** | Gateway、各类 Route（parse 时注册跨 ns 引用） | ReferenceGrantHandler（on_change 时查依赖方） |
| **GatewayRouteIndex** | 各类 Route / EdgionTls（on_change 时写入） | GatewayHandler（on_change 时查关联 Route） |
| **AttachedRouteTracker** | Route（`update_tracker_accepted_only`） | GatewayHandler（统计 listener 挂载路由数） |
| **ListenerPortManager** | GatewayHandler（parse 时注册） | GatewayHandler（冲突检测） |
| **RawStore**（每种资源各一个） | K8s 模式：reflector Store 自动维护；FS 模式：FileSystemResourceController 在 InitApply/Apply/Delete 及 Worker 处理时同步写入 | `lookup_gateway`（route_utils）、`lookup_service_cluster_ip`（gateway.rs）via `ProcessorObj::get_json_by_key` |

---

## 需要关注的问题点

### 1. ConfigMap → Gateway 级联缺失（P3）

Gateway 在 parse 中读取 CA ConfigMap，但未向 SecretRefManager 注册 ConfigMap 键，
导致 ConfigMap 变更时 Gateway 不会被 requeue，需要手动触发或重启才能生效。

### 2. GatewayClass → EdgionGatewayConfig 无校验（P3）

GatewayClass 的 `parametersRef` 指向 EdgionGatewayConfig，但 handler 只做字符串校验，
不通过 PROCESSOR_REGISTRY 验证 EdgionGatewayConfig 是否存在。
