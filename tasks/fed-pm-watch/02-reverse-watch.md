# Phase 2：gRPC 反向 List/Watch PluginMetaData

## 目标

Center 通过已有的 gRPC 双向流，向 Controller 发起 list/watch PluginMetaData 请求。Controller 复用已有的 `ConfigSyncServer` → `ServerCache<PluginMetaData>` 完整 sync_version/EventStore 机制响应。

## 涉及的现有代码

| 角色 | 文件 | 当前状态 |
|------|------|---------|
| proto | `src/core/common/fed_sync/proto/fed_sync.proto` | 需新增消息类型 |
| Controller fed_client | `src/core/controller/fed_sync/fed_client/mod.rs` | 需处理新消息 + 接入 ConfigSyncServer |
| Center gRPC server | `src/core/center/fed_sync/server/mod.rs` | 需发起 watch + 处理 watch 事件 |
| Controller 启动链路 | `src/core/controller/cli/mod.rs` | 需传 ConfigSyncServer 给 fed_client |

## 步骤

### 2.1 Proto 新增消息

在 `fed_sync.proto` 中添加：

```protobuf
// ── Center → Controller 新增 ──

message WatchRequest {
    string request_id    = 1;  // UUID，用于关联
    string kind          = 2;  // "PluginMetaData"
    uint64 from_version  = 3;  // 从哪个 sync_version 开始 watch（0 = 从头）
}

// ── Controller → Center 新增 ──

message WatchListResponse {
    string request_id    = 1;  // echo WatchRequest.request_id
    string data          = 2;  // JSON 序列化的全量 list: [{PM}, {PM}, ...]
    uint64 sync_version  = 3;  // 当前版本
    string server_id     = 4;  // ConfigSyncServer 的 server_id（重启检测）
}

message WatchEventResponse {
    string request_id    = 1;  // echo WatchRequest.request_id
    string data          = 2;  // JSON 序列化的增量事件: [{type, sync_version, data}, ...]
    uint64 sync_version  = 3;  // 当前版本
    string server_id     = 4;  // server_id
    string error         = 5;  // 非空 = 错误（TooOldVersion / EventsLost），需 re-list
}
```

更新 `ControllerMessage.payload`：
```protobuf
message ControllerMessage {
    oneof payload {
        ...                                          // 已有 1-5
        WatchListResponse   watch_list_response  = 6;
        WatchEventResponse  watch_event_response = 7;
    }
}
```

更新 `CenterMessage.payload`：
```protobuf
message CenterMessage {
    oneof payload {
        ...                              // 已有 1-5
        WatchRequest  watch_request  = 6;
    }
}
```

编译：`cargo build` 会自动通过 `build.rs` 重新生成 proto 代码。

### 2.2 Controller fed_client：处理 WatchRequest

**前置**：fed_client 需要拿到 `ConfigSyncServer` 的引用。

#### 2.2.1 修改 fed_client 签名

`connect_and_run()` 和 `handle_center_message()` 需要增加参数：

```rust
// fed_client/mod.rs
use crate::core::controller::conf_sync::ConfigSyncServer;

async fn connect_and_run(
    config: &CenterClientConfig,
    conf_center: Arc<dyn CenterApi>,
    supported_kinds: &[String],
    admin_router: axum::Router,
    config_sync_server: Option<Arc<ConfigSyncServer>>,  // 新增
) -> Result<()> { ... }
```

#### 2.2.2 修改 Controller 启动链路

在 `src/core/controller/cli/mod.rs`（或实际启动 fed_client 的位置），将 `ConfigSyncServer` 传给 `fed_client::run()`。

注意：ConfigSyncServer 可能在 fed_client 启动时还未 ready（资源还在初始加载）。watch 响应需要等 `is_ready()` 为 true。

#### 2.2.3 处理 WatchRequest 消息

在 `handle_center_message()` 中新增分支：

```rust
CenterPayload::WatchRequest(req) => {
    let kind = req.kind.clone();
    let from_version = req.from_version;
    let request_id = req.request_id.clone();

    let Some(css) = config_sync_server.as_ref() else {
        // ConfigSyncServer 不可用，返回错误
        tx.send(ControllerMessage {
            payload: Some(CtrlPayload::WatchEventResponse(WatchEventResponse {
                request_id,
                error: "ConfigSyncServer not available".to_string(),
                ..Default::default()
            })),
        }).await?;
        return Ok(());
    };

    // 如果 from_version == 0，先发 list 全量
    if from_version == 0 {
        match css.list(&kind) {
            Ok(list_data) => {
                tx.send(ControllerMessage {
                    payload: Some(CtrlPayload::WatchListResponse(WatchListResponse {
                        request_id: request_id.clone(),
                        data: list_data.data,
                        sync_version: list_data.sync_version,
                        server_id: list_data.server_id,
                    })),
                }).await?;
            }
            Err(e) => {
                tx.send(ControllerMessage {
                    payload: Some(CtrlPayload::WatchEventResponse(WatchEventResponse {
                        request_id,
                        error: e,
                        ..Default::default()
                    })),
                }).await?;
                return Ok(());
            }
        }
    }

    // 启动 watch 转发任务
    let watch_from = if from_version == 0 {
        // list 之后从 list 返回的 sync_version 开始 watch
        css.list(&kind).map(|l| l.sync_version).unwrap_or(0)
    } else {
        from_version
    };

    match css.watch(&kind, format!("center-{}", request_id), "center".to_string(), watch_from) {
        Ok(mut event_rx) => {
            let tx = tx.clone();
            let request_id = request_id.clone();
            tokio::spawn(async move {
                while let Some(event_data) = event_rx.recv().await {
                    let msg = ControllerMessage {
                        payload: Some(CtrlPayload::WatchEventResponse(WatchEventResponse {
                            request_id: request_id.clone(),
                            data: event_data.data,
                            sync_version: event_data.sync_version,
                            server_id: event_data.server_id,
                            error: event_data.err.unwrap_or_default(),
                        })),
                    };
                    if tx.send(msg).await.is_err() {
                        break;  // gRPC 流关闭
                    }
                }
            });
        }
        Err(e) => {
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::WatchEventResponse(WatchEventResponse {
                    request_id,
                    error: e,
                    ..Default::default()
                })),
            }).await?;
        }
    }
}
```

### 2.3 Center gRPC server：发起 watch + 处理响应

#### 2.3.1 Controller 注册后发起 WatchRequest

在 `src/core/center/fed_sync/server/mod.rs` 中，Controller 完成 `RegisterRequest` 后：

```rust
// 注册完成后，发 WatchRequest
let from_version = pm_store.get_sync_version(&controller_id);
let watch_req = CenterMessage {
    payload: Some(CenterPayload::WatchRequest(WatchRequest {
        request_id: Uuid::new_v4().to_string(),
        kind: "PluginMetaData".to_string(),
        from_version,
    })),
};
stream_tx.send(watch_req).await?;
```

#### 2.3.2 处理 WatchListResponse

收到全量 list 响应：

```rust
CtrlPayload::WatchListResponse(resp) => {
    // 解析 JSON → Vec<PM>
    // 转为 HashMap<pm_key, Value>
    // 调 pm_store.replace_all(controller_id, items, resp.sync_version)
    // 记录 server_id 用于后续变更检测
}
```

#### 2.3.3 处理 WatchEventResponse

收到增量事件：

```rust
CtrlPayload::WatchEventResponse(resp) => {
    if !resp.error.is_empty() {
        // 错误处理：TooOldVersion / EventsLost / ServerReload
        // 重新发 WatchRequest(from_version=0) 触发 re-list
        return;
    }
    // 解析 JSON events → Vec<WatcherEvent>
    // 对每个 event 调 pm_store.apply_event(controller_id, event_type, pm_key, data, sync_version)
}
```

#### 2.3.4 server_id 变更检测

Controller 重启后 `ConfigSyncServer.server_id` 会变。Center 需检测：

```rust
if resp.server_id != stored_server_id {
    // Controller 重启了，之前的 watch 数据可能不连续
    // 重新发 WatchRequest(from_version=0) 触发 re-list
}
```

### 2.4 修改 Controller 启动链路

找到 `fed_client::run()` 的调用点，新增传入 `ConfigSyncServer`：

```rust
// 在 Controller 启动时，ConfigSyncServer 的创建和 fed_client 的启动之间
// 可能需要用 Arc<RwLock<Option<Arc<ConfigSyncServer>>>> 或 watch channel
// 因为 ConfigSyncServer 在资源加载完成后才创建
```

**注意点**：
- `ConfigSyncServer` 的生命周期：在 `KubernetesCenter` / `FileSystemCenter` 的 `start_sync` 流程中创建
- `fed_client` 在更早的时候启动
- 方案：用 `tokio::sync::watch<Option<Arc<ConfigSyncServer>>>` 让 fed_client 等待

### 2.5 Review 检查清单

完成后需确认：
- [ ] proto 编译通过
- [ ] fed_client 能正确响应 WatchRequest（list + watch）
- [ ] Center 收到 list 后 pm_store 数据正确
- [ ] Center 收到 watch event 后增量更新正确
- [ ] Controller 重启（server_id 变化）时 Center 自动 re-list
- [ ] watch 断线重连（from_version > 0）能正确续接
- [ ] watch event 中 error = "TooOldVersion" 时 Center 重新 re-list
- [ ] 多 Controller 同时连接不冲突
- [ ] fed_client 未拿到 ConfigSyncServer 时优雅降级
- [ ] cargo check / cargo clippy 通过
