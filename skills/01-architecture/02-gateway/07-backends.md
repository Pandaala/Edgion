---
name: gateway-backends
description: 后端发现与健康检查：三种发现源、BackendStore、健康检查系统、BackendTLSPolicy、预热与端点验证。
---

# 后端发现与健康检查

> 后端模块负责从 Kubernetes 获取 Service 对应的实际 Pod 端点，
> 并通过健康检查系统自动剔除不健康的后端、恢复健康的后端。

## 三种发现源

| 发现源 | 模块 | K8s 资源 | 适用场景 |
|--------|------|----------|----------|
| Endpoint | `discovery/endpoint/` | `Endpoints` | 传统模式（K8s < 1.21） |
| EndpointSlice | `discovery/endpoint_slice/` | `EndpointSlice` | 推荐模式（K8s >= 1.21），支持更大规模集群 |
| Service | `discovery/services/` | `Service` | Service 元数据存储（ClusterIP、端口映射） |

### EndpointMode 配置

通过 `GLOBAL_ENDPOINT_MODE`（OnceLock，启动时初始化）控制使用哪种发现源：

| 模式 | 说明 |
|------|------|
| `EndpointSlice` | 仅使用 EndpointSlice |
| `Endpoint` | 仅使用 Endpoint |
| `Both` | 同时使用两者 |
| `Auto` | 自动选择（等同 EndpointSlice） |

### 发现数据流

```
Controller (K8s Watch)
  │
  ├── gRPC ConfigSync
  │
  ├── ConfHandler<EndpointSlice>  →  EpSliceStore
  │                                    └── RoundRobin Store (DashMap)
  │                                         └── Pingora LoadBalancer (per service_key)
  │
  ├── ConfHandler<Endpoint>       →  EndpointStore
  │                                    └── RoundRobin Store (DashMap)
  │
  └── ConfHandler<Service>        →  ServiceStore (DashMap)
                                       └── Service 元数据（ports、annotations）
```

每个发现源实现：
- `conf_handler_impl.rs` — ConfHandler trait 实现，处理 full_set/partial_update
- `discovery_impl.rs` — 发现逻辑，将 K8s 资源转换为 Pingora Backend 列表
- `*_store.rs` — 数据存储（EpSliceStore/EndpointStore/ServiceStore）

## 健康检查系统

### 组件结构

```
health/
├── check/
│   ├── manager.rs        # HealthCheckManager — 管理每个 service 的检查任务
│   ├── probes.rs         # 探针实现（HTTP/TCP）
│   ├── config_store.rs   # HealthCheckConfigStore — 健康检查配置存储
│   ├── status_store.rs   # HealthStatusStore — 健康状态存储
│   └── annotation.rs     # Service annotation 解析
└── mod.rs
```

### HealthCheckManager

- 全局单例（`HEALTH_CHECK_MANAGER`，LazyLock）
- 每个 service 维护一个后台 tokio task（`JoinHandle<()>`）
- `reconcile_service(service_key)` — 根据配置创建/更新/删除检查任务
- 配置来源优先级：HealthCheckConfigStore 中的解析结果（annotation 或 CRD）
- 无配置时删除任务并注销健康状态

### 探针类型

| 类型 | 函数 | 说明 |
|------|------|------|
| HTTP | `probe_http()` / `probe_http_with_client()` | HTTP GET 检查，支持自定义 client |
| TCP | `probe_tcp()` | TCP 连接检查 |

### 健康状态

- `HealthStatusStore` 存储每个 service 下各 backend 的健康状态
- 不健康的后端在 LB 选择时被过滤（通过 `health_filter` 回调）
- 后端恢复健康后自动重新加入选择池
- `unregister_service()` 在删除检查配置时清理状态

### backend_hash

用于健康状态键的计算：对 Backend 的 `addr` 和 `weight` 进行 DefaultHasher 哈希。

## BackendTLSPolicy

管理上游 mTLS 配置，存储在 `BackendTLSPolicyStore`：

```
policy/
├── backend_tls/
│   ├── backend_tls_policy_store.rs  # BackendTLSPolicyStore (存储 + 查询)
│   ├── conf_handler_impl.rs         # ConfHandler 实现
│   └── mod.rs
└── mod.rs
```

- 通过 `get_global_backend_tls_policy_store()` 获取全局实例
- 在 `upstream_peer` 阶段查询 service 对应的 TLS 策略
- 配置 HttpPeer 的 TLS 连接参数（CA 证书、客户端证书/密钥）
- Secret key 常量：`CA_CERT`、`CERT`、`KEY`

## LB 预热（Preload）

`preload_load_balancers()` 在所有配置同步完成后调用：

1. 遍历所有路由类型（HTTP/gRPC/TCP/UDP/TLS），收集 (service_key, lb_policy) 对
2. HashSet 去重
3. 根据 EndpointMode 选择 store（EndpointSlice 或 Endpoint）
4. 调用 `get_or_create()` 预创建 RoundRobin LB 实例
5. 日志记录预热结果（total, success, skipped）

## 端点验证

`validate_endpoint_in_route()` 提供安全验证：

- 验证目标 IP 是否是 route 的 BackendRef 中的合法端点
- 防止 SSRF 攻击：确保只能路由到 route 已授权的端点
- 参数：目标 IP、可选端口、route 的 backend_refs、route namespace
- 返回：`Ok((backend_ref_index, port))` 或 `Err(reason)`
- 被 `DirectEndpoint` 和 `DynamicInternalUpstream` 等插件使用

## 目录布局

```
src/core/gateway/backends/
├── mod.rs                    # 模块导出 + EndpointMode 管理
├── discovery/                # 后端发现
│   ├── endpoint/             # Endpoint 发现
│   ├── endpoint_slice/       # EndpointSlice 发现（推荐）
│   └── services/             # Service 元数据
├── health/                   # 健康检查
│   └── check/                # 检查系统
├── policy/                   # 后端策略
│   └── backend_tls/          # BackendTLSPolicy（上游 mTLS）
├── preload.rs                # LB 预热
└── validation.rs             # 端点验证（防 SSRF）
```
