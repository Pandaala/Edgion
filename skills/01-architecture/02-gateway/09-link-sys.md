---
name: gateway-link-sys
description: LinkSys 外部系统集成：5 个 provider、LinkSysStore 管理、DataSender 抽象、ConfHandler 资源更新。
---

# LinkSys 外部系统集成

> LinkSys 是 Edgion 与外部存储/服务集成的统一框架。
> 通过 LinkSys CRD 资源声明外部系统连接，Gateway 自动管理客户端生命周期。

## 5 个 Provider

| Provider | 模块 | 客户端类型 | 连接方式 | 核心功能 |
|----------|------|------------|----------|----------|
| Elasticsearch | `providers/elasticsearch/` | `EsLinkClient` | HTTP bulk API | 批量索引（bulk indexing）、数据发送 |
| Etcd | `providers/etcd/` | `EtcdLinkClient` | gRPC (v3 API) | KV 存储读写 |
| Redis | `providers/redis/` | `RedisLinkClient` | standalone/sentinel/cluster (fred) | 数据读写、分布式限流支持 |
| Webhook | `providers/webhook/` | WebhookManager | HTTP | 外部服务调用、KeyGet::Webhook 解析 |
| LocalFile | `providers/local_file/` | `LocalFileWriter` | 本地文件 I/O | 文件日志（支持 daily/hourly 轮转） |

### Elasticsearch

```
providers/elasticsearch/
├── client.rs          # EsLinkClient（连接管理、健康检查）
├── bulk.rs            # Bulk API 批量操作
├── data_sender.rs     # DataSender<String> 实现
├── config_mapping.rs  # 配置映射
├── ops.rs             # ES 操作封装
└── mod.rs
```

### Redis

```
providers/redis/
├── client.rs          # RedisLinkClient（fred 客户端封装）
├── data_sender.rs     # DataSender<String> 实现
├── config_mapping.rs  # 配置映射（standalone/sentinel/cluster）
├── ops.rs             # Redis 操作封装
└── mod.rs
```

支持三种部署模式：
- **Standalone** — 单节点 Redis
- **Sentinel** — 哨兵高可用
- **Cluster** — Redis Cluster 分片

### Etcd

```
providers/etcd/
├── client.rs          # EtcdLinkClient（etcd-client 封装）
├── config_mapping.rs  # 配置映射
├── ops.rs             # Etcd 操作封装
└── mod.rs
```

### Webhook

```
providers/webhook/
├── manager.rs         # WebhookManager（管理 webhook 实例）
├── runtime.rs         # Webhook 运行时
├── resolver.rs        # KeyGet::Webhook 解析器
├── health.rs          # 健康检查
└── mod.rs
```

### LocalFile

```
providers/local_file/
├── data_sender_impl.rs  # DataSender<String> 实现
├── rotation.rs          # 日志轮转（daily/hourly + 过期清理）
└── mod.rs
```

特点：
- 使用后台线程写入（避免阻塞 tokio runtime）
- `SyncSender<String>` 队列，队列大小默认 `available_cpu_cores * 10,000`
- `LogType` 枚举区分日志类型（Access/Ssl/Tcp/Tls/Udp），用于指标统计

## LinkSysStore

`LinkSysStore` 是所有 LinkSys 资源的统一管理中心：

```rust
pub struct LinkSysStore {
    resources: ArcSwap<LinkSysMap>,  // HashMap<String, LinkSys>
}
```

操作：
- `replace_all(data)` — 全量替换（full sync），原子更新 + 异步分发到各 provider manager
- `update(add_or_update, remove)` — 增量更新（partial sync）
- `get(key)` / `contains(key)` / `count()` — 查询

全局单例：`get_global_link_sys_store()`

### 运行时客户端存储

每种需要持久连接的 provider 有独立的 ArcSwap 运行时 store：

| Provider | 全局 Store | 获取函数 |
|----------|------------|----------|
| Redis | `REDIS_RUNTIME: ArcSwap<HashMap<String, Arc<RedisLinkClient>>>` | `get_redis_client(name)` |
| Etcd | `ETCD_RUNTIME: ArcSwap<HashMap<String, Arc<EtcdLinkClient>>>` | `get_etcd_client(name)` |
| Elasticsearch | `ES_RUNTIME: ArcSwap<HashMap<String, Arc<EsLinkClient>>>` | `get_es_client(name)` |
| Webhook | `WebhookManager`（内部管理） | 通过 `get_webhook_manager()` |

key 格式：`"namespace/name"`

### 客户端生命周期

full_set/partial_update 时的处理流程：
1. 根据 `SystemConfig` 类型分发到对应 provider
2. 创建新客户端实例
3. 先存入 runtime store（确保 `get_*_client()` 立即返回新客户端）
4. 后台 tokio::spawn 初始化连接
5. 后台 tokio::spawn 关闭旧客户端
6. 删除时从所有 provider 的 store 中尝试移除（因为删除事件不携带类型信息）

## DataSender

通用的数据发送 trait，泛型支持不同载荷类型：

```rust
#[async_trait]
pub trait DataSender<T>: Send + Sync where T: Send + 'static {
    async fn init(&mut self) -> Result<()>;
    fn healthy(&self) -> bool;
    async fn send(&self, data: T) -> Result<()>;
    fn name(&self) -> &str;
}
```

- `DataSender<String>` — 文本日志（LocalFile、ES、Redis）
- 支持 FailedCache 模式：ES 不可用时缓存到 LocalFile/Redis，恢复后重放

## ConfHandler

`ConfHandler<LinkSys>` 实现将配置同步系统与 LinkSysStore 连接：

```rust
impl ConfHandler<LinkSys> for Arc<LinkSysStore> {
    fn full_set(&self, data: &HashMap<String, LinkSys>) { ... }
    fn partial_update(&self, add, update, remove) { ... }
}
```

- `create_link_sys_handler()` — 创建 handler 注册到 ConfigClient

## 使用场景

- **AccessLog 输出** — AccessLogger 注册 LocalFileWriter 或 ES DataSender
- **分布式限流** — `rate_limit_redis` 插件通过 `get_redis_client()` 获取 Redis 客户端
- **外部认证** — Webhook provider 支持 KeyGet::Webhook 解析（key_auth 等插件）
- **自定义集成** — 通过 LinkSys CRD 声明连接，业务逻辑获取客户端使用

## 目录布局

```
src/core/gateway/link_sys/
├── mod.rs                    # 模块导出
├── providers/                # 5 个 provider 实现
│   ├── elasticsearch/        # ES 批量索引
│   ├── etcd/                 # Etcd v3 客户端
│   ├── redis/                # Redis 多模式客户端
│   ├── webhook/              # HTTP Webhook
│   ├── local_file/           # 本地文件写入 + 轮转
│   └── mod.rs
└── runtime/                  # 运行时框架
    ├── store.rs              # LinkSysStore + 各 provider 运行时 store
    ├── data_sender.rs        # DataSender trait 定义
    ├── conf_handler.rs       # ConfHandler<LinkSys> 实现
    └── mod.rs
```
