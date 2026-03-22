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
| `controller_id` | `String` | 每次启动重新生成的唯一 ID（毫秒时间戳或 UUID） |
| `cluster` | `String` | 集群标识，固定 string，center 按此分组 |
| `env` | `[]String` | 环境标签，如 `["production"]`，扩展用 |
| `tag` | `[]String` | 自定义标签，扩展用 |
| `supported_kinds` | `[]String` | 本 controller 持有的资源 kind 列表 |

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
    string          controller_id   = 1;  // 每次启动重新生成的唯一 ID
    string          cluster         = 2;  // 集群标识（固定 string）
    repeated string env             = 3;  // 环境标签列表
    repeated string tag             = 4;  // 扩展标签列表
    repeated string supported_kinds = 5;  // 本 controller 持有的资源 kind 列表
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
    string session_id = 1;  // center 分配的会话 ID
}

message Ping {
    uint64 timestamp = 1;  // 毫秒时间戳
}

message ListRequest {
    string          request_id = 1;  // 用于匹配响应
    repeated string kinds      = 2;  // 空 = 所有 supported_kinds（自动排除 no_sync_kinds）
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
| `center/fed_sync/server` | 接收 controller 连接，路由消息到各子模块 |
| `center/fed_sync/registry` | 管理 session 生命周期，cluster 分组索引 |
| `center/aggregator` | 维护每个 controller 的资源 key 内存快照，list 响应到来时全量替换 |
| `center/scheduler` | 每 5min 遍历所有在线 controller，通过 registry 发出 ListRequest |
| `center/commander` | 提供接口供 Admin API 调用，将命令写入对应 controller 的 stream |

---

## 7. 数据流

### 正常连接流程

```
Controller                                    Center
    │                                            │
    │── Connect(stream) ────────────────────────►│ registry 记录 stream handle
    │── RegisterRequest{cluster,env,tag,kinds} ─►│ 分配 session_id
    │◄─ RegisterAck{session_id} ─────────────────│
    │                                            │
    │  [心跳循环，center 每 30s 发一次]           │
    │◄─ Ping{timestamp} ──────────────────────── │
    │── Pong{timestamp} ────────────────────────►│ 更新 last_seen
    │                                            │
    │  [scheduler 每 5min 触发]                  │
    │◄─ ListRequest{request_id, kinds:[]} ────── │
    │── ListResponse{request_id, keys:[...]} ───►│ aggregator 全量替换该 controller 快照
    │                                            │
    │  [Admin API 触发命令]                       │
    │◄─ CommandRequest{reload} ───────────────── │
    │── CommandResponse{success:true} ──────────►│
```

### Aggregator 数据结构（内存）

```
controller_id → {
    info: RegisterRequest,       // cluster / env / tag / supported_kinds
    session_id: String,
    last_list_at: Instant,       // 最后一次成功 list 的时间
    online: bool,
    kinds: Map<Kind, Vec<ResourceKey>>
}
```

离线 controller 的数据保留（`online=false`），`last_list_at` 供查询方判断数据新鲜度。

---

## 8. 错误处理

| 场景 | 处理方式 |
|------|---------|
| **心跳超时**（center 侧） | 超过 `3 × ping_interval`（默认 90s）未收到 Pong，center 主动关闭 stream，registry 标记 `online=false`，aggregator 保留最后快照并标注 stale |
| **controller 断线重连** | 重走 Connect → RegisterRequest；center 用 `controller_id` 匹配旧 session，清理后建立新 session；aggregator 等待下次 ListRequest 刷新 |
| **ListRequest 超时** | 发出后 30s 无响应，记录 warn 日志，本次跳过；不断开连接，等下个周期重试 |
| **CommandRequest 超时** | 30s 无 CommandResponse，向 Admin API 调用方返回超时错误 |
| **center 不可达（controller 侧）** | 指数退避重连（1s → 2s → ... → 60s 上限），不阻塞 controller 主流程启动 |
| **center 重启** | controller 检测 stream 断开后自动重连，重新注册；center 冷启动内存清空，等各 controller 重连后快照自然恢复 |

---

## 9. Controller 配置（TOML）

```toml
# 未配置此块则 fed_sync 模块完全不启动
[center]
address = "https://center.example.com:50052"
cluster = "prod-cn-north"
env     = ["production"]
tag     = ["team-infra"]
```

---

## 10. 不在本期范围

- 认证鉴权（mTLS / Token）— 预留接口，后续扩展
- 资源持久化 — center 重启后依赖 controller 重连恢复
- Controller push 推送模式 — 当前仅 center 定期 pull
- Full raw 资源同步 — 当前仅同步 ResourceKey（元信息）
- Center 高可用 — 单实例，后续按需扩展
