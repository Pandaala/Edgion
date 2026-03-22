# gRPC Health Check Protocol 支持 — 研究报告

## 1. 当前健康检查机制

### 1.1 整体架构

系统采用四层架构，彼此职责分离：

- **`health_check.rs`**（类型层）：定义 `ServiceHealthCheck`、`ActiveHealthCheckConfig`、`HealthCheckType` 等数据结构。
- **`config_store.rs`**（配置层）：按 service key 存储健康检查配置，支持三级优先级（EndpointSlice > Endpoint > Service）。
- **`manager.rs`**（调度层）：按 service 维护后台 Tokio 任务，配置变更时 abort 旧任务并启动新任务，每个任务运行 `health_check_loop`。
- **`status_store.rs`**（状态层）：存储每个 backend 的探针计数器、健康状态快照；request path 通过 `is_healthy()` 快速查询（无锁热路径）。

### 1.2 当前支持的探针类型

**`HealthCheckType` 枚举**（`src/types/resources/health_check.rs`）：
```rust
pub enum HealthCheckType {
    Http,
    Tcp,
}
```

**HTTP 探针**（`probe_http_with_client`）：
- 复用 `reqwest::Client`（在 loop 初始化，失败时降级为 per-probe client）。
- 参数：addr、path、port_override、host header、expected_statuses。
- 错误：Timeout、ConnectionRefused、UnexpectedStatus、RequestFailed、InvalidPath。

**TCP 探针**（`probe_tcp`）：
- 仅尝试 TCP 连接，成功即为健康。
- 参数：addr、port_override、timeout。

### 1.3 配置方式

通过 Kubernetes annotation `edgion.io/health-check` 注入 YAML：

```yaml
# Service/EndpointSlice/Endpoint 上的 annotation
edgion.io/health-check: |
  active:
    type: http
    path: /healthz
    port: 8080
    interval: 10s
    timeout: 3s
    healthyThreshold: 2
    unhealthyThreshold: 3
    expectedStatuses: [200, 204]
    host: backend.internal
```

`ActiveHealthCheckConfig` 字段（camelCase serde）：
| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| type | HealthCheckType | http | 探针类型 |
| path | Option\<String\> | "/" | HTTP 路径 |
| port | Option\<u16\> | - | 端口覆盖 |
| interval | String | "10s" | 探针间隔 |
| timeout | String | "3s" | 探针超时 |
| healthyThreshold | u32 | 2 | 连续成功次数 |
| unhealthyThreshold | u32 | 3 | 连续失败次数 |
| expectedStatuses | Vec\<u16\> | [200] | 期望 HTTP 状态码 |
| host | Option\<String\> | - | Host 头覆盖 |

### 1.4 manager 中的调度流程

`health_check_loop` 核心逻辑：
1. 解析 interval、timeout 等参数（含保底最小值）。
2. 若类型为 Http，预先构建可复用的 `reqwest::Client`。
3. 基于 service_key hash 计算启动抖动（最大 interval/2），避免探针风暴。
4. 进入 ticker 循环：获取 backends → 注册到 status_store → 逐 backend 探针 → `record_check`。
5. `match config.r#type { Http => ..., Tcp => ... }` 分支选择探针。

## 2. gRPC 探针设计

### 2.1 函数签名

参照现有探针的设计模式，gRPC 探针应分为两个函数：

```rust
// 构建可复用的 gRPC channel（类比 build_http_probe_client）
pub async fn build_grpc_health_channel(
    addr: SocketAddr,
    timeout: Duration,
) -> Result<tonic::transport::Channel, tonic::transport::Error>;

// 执行一次 gRPC Health.Check 调用（类比 probe_http_with_client）
pub async fn probe_grpc_with_channel(
    channel: &tonic::transport::Channel,
    service_name: &str,  // 空字符串 = 检查整个服务器
) -> Result<(), ProbeError>;

// 一次性探针（类比 probe_tcp，含内部建连）
pub async fn probe_grpc(
    addr: SocketAddr,
    port_override: Option<u16>,
    service_name: &str,
    timeout: Duration,
) -> Result<(), ProbeError>;
```

### 2.2 错误类型扩展

需在 `ProbeError` 枚举中新增：
```rust
pub enum ProbeError {
    Timeout,
    ConnectionRefused(std::io::Error),
    UnexpectedStatus(u16),
    RequestFailed(reqwest::Error),
    InvalidPath,
    // 新增
    GrpcConnectionFailed(tonic::transport::Error),
    GrpcNotServing(i32),   // ServingStatus 枚举值（非 SERVING）
    GrpcCallFailed(tonic::Status),
}
```

### 2.3 连接管理策略

与 HTTP 探针类似，在 `health_check_loop` 初始化阶段为 gRPC 类型预建 Channel，保存在循环外部：

```rust
let mut grpc_channel: Option<tonic::transport::Channel> = if matches!(config.r#type, HealthCheckType::Grpc) {
    // 预建 channel（懒连接，tonic channel 本身是延迟连接）
    build_grpc_health_channel(/* first backend addr or reuse per-probe */).await.ok()
} else {
    None
};
```

**关键决策**：gRPC Channel 与 HTTP Client 有所不同。
- `reqwest::Client` 是无地址状态的连接池，可在所有 backend 之间复用。
- `tonic::transport::Channel` 绑定到特定 endpoint URI。
- 因此对 gRPC 探针，应**每个 backend 独立建立 Channel**，或者采用**per-probe 建连**（简单但开销大）。

推荐方案：**使用 `HashMap<u64, Channel>` 缓存每个 backend 的 channel**，在 loop 内惰性初始化，backend 消失时自动淘汰。这与 status_store 中 backend 的生命周期对齐。

```rust
let mut grpc_channels: HashMap<u64, tonic::transport::Channel> = HashMap::new();

// 循环内，每 backend 处理：
let channel = grpc_channels.entry(hash).or_insert_with(|| {
    build_grpc_health_channel_sync(target_addr, timeout)
});
```

注意：tonic Channel 创建本身是同步的（懒连接），`connect()` 调用才真正建连。可用 `Channel::from_shared(uri).unwrap().connect_lazy()` 实现惰性连接。

### 2.4 proto 处理

有两种方式处理 `grpc.health.v1.Health` proto：

**方案 A（推荐）：手写最简客户端，不用 proto 文件**

gRPC Health Check 协议的 wire format 极简（service 字符串 field 1，status int32 field 1），可以用 `prost` 直接定义结构体，避免引入额外 proto 文件和 build.rs 复杂度：

```rust
// 在 probes.rs 或新建 grpc_health_proto.rs 中
mod health_proto {
    #[derive(prost::Message)]
    pub struct HealthCheckRequest {
        #[prost(string, tag = "1")]
        pub service: String,
    }
    #[derive(prost::Message)]
    pub struct HealthCheckResponse {
        #[prost(int32, tag = "1")]
        pub status: i32,
    }
}
```

然后用 `tonic::client::Grpc` 低级 API 发起调用，无需 codegen。

**方案 B：添加 proto 文件，扩展 build.rs**

新建 `src/core/gateway/backends/health/check/proto/health.proto`，在 `build.rs` 中新增编译步骤。代码更清晰，但 build.rs 需改动，且生成代码体积稍大。

推荐 **方案 A**，因为该 proto 是标准不变协议，手写 prost 宏更轻量，不影响 build.rs。

## 3. 配置扩展方案

### 3.1 `HealthCheckType` 枚举扩展

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthCheckType {
    Http,
    Tcp,
    Grpc,  // 新增
}
```

### 3.2 `ActiveHealthCheckConfig` 新增字段

`grpcServiceName` 是 gRPC 探针特有的配置，加在 `ActiveHealthCheckConfig` 中（其他类型忽略）：

```rust
pub struct ActiveHealthCheckConfig {
    // ... 现有字段 ...

    /// gRPC Health Check 协议的 service name。
    /// 空字符串（默认）表示检查整个服务器。
    /// 仅在 type: grpc 时生效。
    #[serde(default)]
    pub grpc_service_name: Option<String>,
}
```

序列化 key 为 `grpcServiceName`（camelCase）。

### 3.3 annotation 示例

```yaml
edgion.io/health-check: |
  active:
    type: grpc
    port: 9090
    interval: 10s
    timeout: 3s
    healthyThreshold: 2
    unhealthyThreshold: 3
    grpcServiceName: "my.package.MyService"  # 可选，默认为空
```

### 3.4 校验规则

在 `ServiceHealthCheck::get_validation_error()` 中新增：
- 若 `type: grpc`，`grpcServiceName` 可以为空（表示检查整个服务器）；
- 不需要强制 path 或 expectedStatuses（这些字段对 gRPC 无意义，校验时跳过）。

## 4. 依赖分析

### 4.1 已有依赖（无需新增）

`Cargo.toml` 已包含：
- `tonic = { version = "0.12", features = ["tls", "tls-webpki-roots"] }` — gRPC 客户端框架。
- `prost = "0.13"` — protobuf 序列化/反序列化。
- `tonic-build = "0.12"` — build-dependency，proto 编译工具（build.rs 已用于 config_sync 和 fed_sync）。
- `tokio = { version = "1.49", features = ["full"] }` — 异步运行时。

**结论：无需添加任何新依赖。** 项目已完整具备实现 gRPC Health Check 探针所需的全部库。

### 4.2 TLS 考量

现有 tonic 依赖已启用 TLS（`tls-webpki-roots`），但健康检查场景通常连 cluster 内部裸 gRPC（明文），不需要 TLS。使用 `http://` scheme 建立 channel 即可，无需额外配置。

## 5. 实现路径

按依赖顺序列出需要改动的文件：

### Step 1：类型扩展（`src/types/resources/health_check.rs`）

1. 在 `HealthCheckType` 枚举加 `Grpc` 变体。
2. 在 `ActiveHealthCheckConfig` 加 `grpc_service_name: Option<String>` 字段（默认 `None`）。
3. 更新 `ServiceHealthCheck::get_validation_error()`：对 `Grpc` 类型不校验 `path` 和 `expectedStatuses`。
4. 更新 `Default` impl（`grpc_service_name: None`）。
5. 补充测试：grpc 类型的反序列化、校验。

### Step 2：探针实现（`src/core/gateway/backends/health/check/probes.rs`）

1. 在 `ProbeError` 加 `GrpcConnectionFailed`、`GrpcNotServing`、`GrpcCallFailed` 变体。
2. 在文件顶部或新建子模块 `grpc_health_proto` 定义 `HealthCheckRequest`/`HealthCheckResponse` prost 结构体。
3. 实现 `probe_grpc(addr, port_override, service_name, timeout) -> Result<(), ProbeError>`。
4. 内部：构建 tonic Channel（`connect_lazy`）→ 创建 `tonic::client::Grpc` 客户端 → 编码请求 → 调用 `/grpc.health.v1.Health/Check` → 解析响应 status（`SERVING = 1` 为健康）。
5. 补充单元测试（mock gRPC 服务器或用 `tonic` 测试工具）。

### Step 3：manager 集成（`src/core/gateway/backends/health/check/manager.rs`）

1. 在 `health_check_loop` 初始化段：为 `Grpc` 类型初始化 `grpc_channels: HashMap<u64, Channel>`（或更简单：per-probe 建连，权衡复杂度与性能）。
2. 在 `match config.r#type { ... }` 分支新增：
   ```rust
   HealthCheckType::Grpc => {
       probe_grpc(*addr, config.port, grpc_service_name, timeout).await
   }
   ```
3. 从 config 提取 `grpc_service_name`（`config.grpc_service_name.as_deref().unwrap_or("")`）。
4. 更新 import：添加 `probe_grpc`。

### Step 4：annotation 文档更新（可选）

更新项目 docs 或 annotation schema 示例，展示 gRPC 探针配置格式。

## 6. 实现难度评估

| 模块 | 改动量 | 复杂度 | 说明 |
|------|--------|--------|------|
| `health_check.rs` 类型扩展 | ~30 行 | 低 | 加字段、加枚举变体、更新校验 |
| `probes.rs` gRPC 探针 | ~80 行 | 中 | 主要复杂度在 tonic 低级 API 使用和 proto 手写 |
| `manager.rs` 集成 | ~20 行 | 低 | 在现有 match 分支加一项，连接复用可后续优化 |
| 测试 | ~60 行 | 中 | 需要 mock gRPC 健康检查服务器（可用 tonic 构建）|

**总工作量估计**：约 1-2 个工作日（不含联调和集成测试）。

整体难度：**中低**。最大的技术风险点是 tonic 低级客户端 API（`tonic::client::Grpc`）的使用方式，需要查阅 tonic 0.12 文档确认 codec 和 path 的正确写法。

## 7. 下一步 — 第一个 PR 的具体范围

**PR 范围：最小可用实现（MVP）**，包含：

1. `src/types/resources/health_check.rs`：
   - 加 `HealthCheckType::Grpc`。
   - 加 `ActiveHealthCheckConfig.grpc_service_name: Option<String>`。
   - 更新 `get_validation_error()`（跳过 HTTP 特有校验）。
   - 补充类型测试。

2. `src/core/gateway/backends/health/check/probes.rs`：
   - 扩展 `ProbeError`。
   - 手写 prost Health proto 结构体（`HealthCheckRequest`/`HealthCheckResponse`）。
   - 实现 `probe_grpc()`（per-probe 建连，简单可靠，复用优化留后续 PR）。
   - 补充探针测试。

3. `src/core/gateway/backends/health/check/manager.rs`：
   - `health_check_loop` 中增加 `Grpc` 分支调用 `probe_grpc()`。
   - 无连接复用（MVP 阶段接受 per-probe 建连开销）。

**不在本 PR 范围**：
- Channel 复用优化（每 backend 缓存 Channel）。
- TLS 支持（cluster 内通常明文，后续按需）。
- gRPC Watch 流式健康检查（标准 Watch RPC，复杂度高，优先级低）。
- 文档和 annotation schema 更新。

**PR 验证方式**：
- 在 kind/k3d 集群中部署一个暴露 `grpc.health.v1.Health` 的 gRPC 服务（如 grpc-health-probe 测试服务）。
- 配置 annotation `type: grpc`，观察健康状态切换是否正确。
- 停止服务后确认 backend 被标记为 unhealthy，重启后恢复 healthy。
