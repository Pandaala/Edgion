# Federated Center 设计文档

**日期**: 2026-03-22
**状态**: 待实现
**涉及 Binary**: `edgion-center`（新增）、`edgion-controller`（扩展）

---

## 1. 背景与目标

在多集群场景下，需要一个统一的联邦 center 来管理多个 controller。目标：

- Controller 主动注册到 center，center 是统一入口，单向可通（controller → center）
- Center 定期聚合所有 controller 的资源 key（元信息），形成全局只读视图
- Center 可向 controller 下发命令（reload / apply / delete）和资源
- Controller 侧通过可选配置块启用，未配置则完全无感知

**不在本期范围**：认证鉴权、资源持久化、controller push 推送模式。

---

## 2. 整体架构

```
Controller (gRPC client)                      Center (gRPC server)
┌──────────────────────────────┐             ┌──────────────────────────────────┐
│ [center] config (可选)       │             │ FederationServer                 │
│  enabled → 启动 FedClient    │             │                                  │
│                              │  Connect()  │ ControllerRegistry               │
│ FederationClient             │────────────►│  cluster → [controller_a, ...]   │
│  ├── 第1条消息: Register     │             │  session_id → stream handle      │
│  │   (cluster/env/tag/kinds) │◄───────────►│                                  │
│  ├── 心跳 Ping/Pong          │  双向stream │ ResourceAggregator (内存)        │
│  ├── ListResponse (响应)     │             │  per-controller, per-kind keys   │
│  └── CommandResponse         │             │                                  │
│                              │             │ Scheduler (5min 定期)            │
│ ConfCenter (原始资源来源)    │             │  → 向各 controller 发 ListRequest│
│ CommandExecutor              │             │                                  │
│  (apply/delete/reload)       │             │ CommandDispatcher                │
└──────────────────────────────┘             │                                  │
                                             │ Admin API                        │
                                             │  (查询聚合视图、下发命令)         │
                                             └──────────────────────────────────┘
```

**核心流程**：
1. Controller 配置了 `[center]` 才启动 `FederationClient`，否则完全无感知
2. 连接后第一条消息即为注册信息（cluster/env/tag/supported_kinds），折叠进 stream
3. 之后保持心跳 Ping/Pong（30s 间隔），center 侧检测超时即标记 controller 离线
4. Center Scheduler 每 5min 向所有在线 controller 发 `ListRequest`，controller 返回资源 key 列表
5. Center 可随时通过 stream 下发命令，controller 执行后回复 `CommandResponse`
6. 断线后 controller 自动指数退避重连，重新发注册消息，center 清理旧 session

---

## 3. Controller 注册元信息

Controller 注册时携带以下字段用于 center 侧分组和查询：

| 字段 | 类型 | 说明 |
|------|------|------|
| `controller_id` | `String` | **稳定标识**，由 `cluster` + `[center].name`（配置项）拼接生成，跨重启不变，用于 center 匹配旧 session |
| `cluster` | `String` | 集群标识，固定 string，center 按此分组 |
| `env` | `[]String` | 环境标签，如 `["production"]`，扩展用 |
| `tag` | `[]String` | 自定义标签，扩展用 |
| `supported_kinds` | `[]String` | 本 controller 持有的资源 kind 列表（**已排除 no_sync_kinds**，controller 侧过滤后上报） |

---

## 4. Proto 定义

文件路径：`src/core/common/fed_sync/proto/fed_sync.proto`

```protobuf
syntax = "proto3";

package fed_sync;

// Controller (client) 连接 Center (server) 的持久双向流
// Controller 第一条消息必须是 RegisterRequest
service FederationSync {
    rpc Connect(stream ControllerMessage) returns (stream CenterMessage);
}

// ── Controller → Center ──────────────────────────────────────

message ControllerMessage {
    oneof payload {
        RegisterRequest register         = 1;  // 首条消息，注册自身信息
        Pong            pong             = 2;  // 心跳响应
        ListResponse    list_response    = 3;  // 响应 center 的 ListRequest
        CommandResponse command_response = 4;  // 响应 center 的命令
    }
}

message RegisterRequest {
    string          controller_id   = 1;  // 稳定标识：cluster + name 拼接，跨重启不变
    string          cluster         = 2;  // 集群标识（固定 string）
    repeated string env             = 3;  // 环境标签列表
    repeated string tag             = 4;  // 扩展标签列表
    repeated string supported_kinds = 5;  // 本 controller 持有的资源 kind 列表（已排除 no_sync_kinds）
}

message Pong {
    uint64 timestamp = 1;  // echo Ping.timestamp，毫秒时间戳
}

message ListResponse {
    string               request_id = 1;  // echo ListRequest.request_id
    repeated ResourceKey keys       = 2;
}

// 资源 key：只含元信息，无原始 spec/status
message ResourceKey {
    string              kind             = 1;
    string              namespace        = 2;
    string              name             = 3;
    string              resource_version = 4;
    map<string, string> labels           = 5;
    map<string, string> annotations      = 6;
}

message CommandResponse {
    string request_id = 1;
    bool   success    = 2;
    string message    = 3;  // 失败时的错误信息
}

// ── Center → Controller ──────────────────────────────────────

message CenterMessage {
    oneof payload {
        RegisterAck    register_ack = 1;  // 注册确认
        Ping           ping         = 2;  // 心跳
        ListRequest    list_request = 3;  // 定期拉取资源 key
        CommandRequest command      = 4;  // 下发命令
    }
}

message RegisterAck {
    // session_id 是 center 内部概念，仅用于日志追踪，controller 不需要回传
    string session_id = 1;
}

message Ping {
    uint64 timestamp = 1;  // 毫秒时间戳
}

message ListRequest {
    string          request_id = 1;  // 由 scheduler 生成（UUID），server 维护 pending map 做响应关联
    repeated string kinds      = 2;  // 空 = 所有 supported_kinds（controller 侧已排除 no_sync_kinds）
}

message CommandRequest {
    string request_id = 1;
    oneof command {
        ReloadCommand reload = 2;
        ApplyCommand  apply  = 3;
        DeleteCommand delete = 4;
    }
}

message ReloadCommand {}

message ApplyCommand {
    string kind = 1;
    string data = 2;  // 资源 JSON（ConfCenter 原始格式）
}

message DeleteCommand {
    string kind      = 1;
    string namespace = 2;
    string name      = 3;
}
```

---

## 5. no_sync_kinds（联邦侧）

以下资源不通过联邦 center 同步，`resource_collector` 在 controller 侧过滤：

| 资源 | 原因 |
|------|------|
| `Secret` | 含敏感信息（证书/密钥） |
| `ConfigMap` | 含敏感或大量配置数据 |
| `Endpoint` | 高频变更，元信息价值低 |
| `EndpointSlice` | 同上 |
| `ReferenceGrant` | 仅 controller 侧跨命名空间引用校验使用 |

---

## 6. 模块结构

```
src/core/
├── common/
│   └── fed_sync/                 # 新增：proto + 共享类型
│       ├── proto/
│       │   └── fed_sync.proto
│       └── types/                # ControllerMessage / CenterMessage 等生成类型
│
├── controller/
│   └── fed_sync/                 # 新增：controller 侧 FederationClient
│       ├── fed_client/           # gRPC 连接、注册、心跳、断线重连
│       └── resource_collector/   # 从 ConfCenter 读原始资源 → ResourceKey 列表，过滤 no_sync_kinds
│
└── center/                       # 新增顶级组（对应 edgion-center binary）
    ├── cli/                      # CLI 入口、启动、配置加载
    ├── api/                      # Admin API（查询聚合视图、下发命令）
    ├── fed_sync/                 # FederationServer + 连接管理
    │   ├── server/               # gRPC 服务端实现（Connect RPC）
    │   └── registry/             # ControllerRegistry（session → stream handle）
    ├── aggregator/               # ResourceAggregator（内存镜像）
    ├── scheduler/                # 定时调度（5min 发 ListRequest）
    └── commander/                # CommandDispatcher（向指定 controller 下发命令）
```

**各模块职责**：

| 模块 | 职责 |
|------|------|
| `common/fed_sync` | proto + 共享消息类型，controller 和 center 均依赖 |
| `controller/fed_sync/fed_client` | 连接 center、发 Register、维持心跳、响应 ListRequest 和 Command |
| `controller/fed_sync/resource_collector` | 从 ConfCenter 读取原始资源，过滤 no_sync_kinds，返回 ResourceKey |
| `center/fed_sync/server` | 接收 controller 连接；强制要求第一条消息为 RegisterRequest（5s 超时），之后路由消息到各子模块；维护 pending ListRequest map（`HashMap<request_id, oneshot::Sender>`）做响应关联 |
| `center/fed_sync/registry` | 管理 session 生命周期（`controller_id → session`），cluster 分组索引；`session_id` 是内部概念，仅用于日志 |
| `center/aggregator` | 维护每个 controller 的资源 key 内存快照，list 响应到来时全量替换；离线条目保留 24h 后自动清除 |
| `center/scheduler` | 每 5min 遍历所有在线 controller，生成 UUID 作为 `request_id`，通过 registry 发出 ListRequest，并将 `request_id` 注册到 server 的 pending map |
| `center/commander` | 提供接口供 Admin API 调用（HTTP 同步，最长等待 30s），将命令写入对应 controller 的 stream |

---

## 7. 数据流

### 正常连接流程

```
Controller                                    Center
    │                                            │
    │── Connect(stream) ────────────────────────►│ server 等待首条消息（5s 超时）
    │── RegisterRequest{cluster,env,tag,kinds} ─►│ 注册 session，registry 建立索引
    │◄─ RegisterAck{session_id} ─────────────────│ （session_id 仅供日志，controller 无需回传）
    │                                            │
    │  [心跳循环，center 每 30s 发一次]           │
    │◄─ Ping{timestamp} ──────────────────────── │
    │── Pong{timestamp} ────────────────────────►│ 更新 last_seen
    │                                            │
    │  [scheduler 每 5min 触发]                  │
    │  scheduler 生成 request_id(UUID)            │
    │  注册到 server pending map                  │
    │◄─ ListRequest{request_id, kinds:[]} ────── │
    │── ListResponse{request_id, keys:[...]} ───►│ server 从 pending map 取 sender
    │                                            │ aggregator 全量替换该 controller 快照
    │  [Admin API 触发命令，HTTP 同步等待 30s]    │
    │◄─ CommandRequest{request_id, reload} ───── │
    │── CommandResponse{success:true} ──────────►│ commander 返回 HTTP 响应
```

### Aggregator 数据结构（内存）

```
controller_id → {
    info: RegisterRequest,       // cluster / env / tag / supported_kinds
    last_list_at: Instant,       // 最后一次成功 list 的时间
    offline_since: Option<Instant>, // 首次离线时间，None 表示在线
    kinds: Map<Kind, Vec<ResourceKey>>
}
```

离线 controller 的数据保留（`offline_since.is_some()`），`last_list_at` 供查询方判断数据新鲜度。离线超过 **24h** 的条目自动清除，避免内存无限增长。

---

## 8. 错误处理

| 场景 | 处理方式 |
|------|---------|
| **注册超时**（center 侧） | Connect 后 5s 内未收到 RegisterRequest，server 关闭 stream，不建立 session |
| **心跳超时**（center 侧） | 超过 `3 × ping_interval`（默认 90s）未收到 Pong，center 主动关闭 stream，aggregator 设置 `offline_since`，保留最后快照 |
| **controller 断线重连** | 重走 Connect → RegisterRequest；center 用 `controller_id`（稳定标识）匹配旧条目，清理旧 stream，重置 `offline_since=None`；aggregator 等待下次 ListRequest 刷新 |
| **ListRequest 超时** | 发出后 30s 无响应，server 从 pending map 移除该 request_id，scheduler 记录 warn 日志，本次跳过；不断开连接，等下个周期重试 |
| **CommandRequest 超时** | 30s 无 CommandResponse，commander 从 pending map 移除，向 Admin API 调用方返回 HTTP 504 |
| **center 不可达（controller 侧）** | 指数退避重连（1s → 2s → ... → 60s 上限），不阻塞 controller 主流程启动 |
| **center 重启** | controller 检测 stream 断开后自动重连，重新注册；center 冷启动内存清空，等各 controller 重连后快照自然恢复 |
| **离线超 24h** | aggregator 清除该 controller 的全部缓存条目，释放内存 |

---

## 9. 配置（TOML）

### Controller 侧

```toml
# 未配置此块则 fed_sync 模块完全不启动
[center]
address = "https://center.example.com:50052"
name    = "controller-01"      # 与 cluster 拼接生成稳定 controller_id，跨重启不变
cluster = "prod-cn-north"
env     = ["production"]
tag     = ["team-infra"]
# 可选，使用默认值时无需填写：
# ping_interval_secs = 30     # 心跳间隔，默认 30s
```

### Center 侧（edgion-center）

```toml
[server]
grpc_addr = "0.0.0.0:50052"   # FederationSync gRPC 监听地址
http_addr = "0.0.0.0:5810"    # Admin API HTTP 监听地址

[sync]
list_interval_secs      = 300  # 定期 list 间隔，默认 5min
list_timeout_secs       = 30   # 单次 ListRequest 超时
command_timeout_secs    = 30   # CommandRequest 超时
ping_interval_secs      = 30   # 下行心跳间隔
offline_evict_hours     = 24   # 离线 controller 数据保留时长
```

---

## 10. 不在本期范围

- 认证鉴权（mTLS / Token）— 预留接口，后续扩展
- 资源持久化 — center 重启后依赖 controller 重连恢复
- Controller push 推送模式 — 当前仅 center 定期 pull
- Full raw 资源同步 — 当前仅同步 ResourceKey（元信息）
- Center 高可用 — 单实例，后续按需扩展
