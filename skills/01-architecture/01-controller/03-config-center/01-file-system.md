---
name: config-center-file-system
description: FileSystemCenter 实现：本地 YAML 目录监听、文件命名规范、status 持久化、启动扫描与运行时监听。
---

# FileSystemCenter 实现

FileSystemCenter 是面向开发和测试场景的配置中心后端，监听本地 YAML 目录，
将文件变更转化为资源事件，经过与 KubernetesCenter 完全相同的 Workqueue + ResourceProcessor 管线处理。

## 架构

```
FileSystemCenter
├── writer: FileSystemStorage       # CenterApi 委托（CRUD 操作）
├── config: FileSystemConfig        # 配置（conf_dir, endpoint_mode 等）
├── config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>
├── reload_tx: Mutex<Option<mpsc::Sender<()>>>
└── controller_handle: Mutex<Option<JoinHandle<()>>>

FileSystemController.run()
├── FileSystemWatcher（共享，集中式文件监听）
│   ├── [inotify/kqueue] → [debouncer] → [dispatcher] → [kind channels]
│   │                                        ├── HTTPRoute channel
│   │                                        ├── Gateway channel
│   │                                        └── ... 其他 kind channels
│
├── spawn::<HTTPRoute, _>(HttpRouteHandler)
│   ├── ResourceProcessor + 注册到 PROCESSOR_REGISTRY
│   └── FileSystemResourceController（订阅 kind channel）
│
├── spawn::<Gateway, _>(GatewayHandler)
│   └── ...
└── 约 20 种资源类型，全部独立并行
```

## 文件命名规范

所有资源文件存放在同一个平坦目录下，通过文件名编码资源元数据：

| 资源作用域 | 文件名格式 | 示例 |
|-----------|-----------|------|
| 命名空间级 | `{Kind}_{namespace}_{name}.yaml` | `HTTPRoute_default_my-route.yaml` |
| 集群级 | `{Kind}__{name}.yaml`（双下划线） | `GatewayClass__edgion.yaml` |

Status 文件：在对应配置文件名后追加 `.status` 后缀。

| 配置文件 | Status 文件 |
|---------|------------|
| `HTTPRoute_default_my-route.yaml` | `HTTPRoute_default_my-route.yaml.status` |
| `GatewayClass__edgion.yaml` | `GatewayClass__edgion.yaml.status` |

解析规则：`parse_resource_filename()` 按第一个 `_` 分割为最多 3 段（`splitn(3, '_')`）：
- 第一段为 Kind
- 第二段为空字符串（双下划线）时表示集群级资源，否则为 namespace
- 第三段为 name

## Status 持久化

FileSystemCenter 不调用 K8s API，而是将 status 写入本地 `.status` 文件：

- **格式**：原生 status 结构的 YAML 序列化（保留完整的 Gateway API status 结构）
- **写入时机**：ResourceProcessor 处理完成后，status 发生变化时写入
- **读取用途**：status 变更检测（读取后转为 JSON 字符串比较）
- **错误 status**：资源解析失败时写入简化的 `ErrorStatus`（`conditions: [{type: Ready, status: False, reason: ParseError}]`）
- **孤儿清理**：启动时 `cleanup_orphans()` 扫描并删除没有对应配置文件的 `.status` 文件

## 启动流程

FileSystemCenter 的 `start()` 方法执行以下步骤（支持 reload 循环）：

```
start(shutdown_handle)
└── loop:
    ├── 1. 创建 iteration_shutdown + reload channel + error channel
    ├── 2. FileSystemController.run()
    │   ├── 清理孤儿 .status 文件
    │   ├── Phase 1: spawn 基础资源控制器
    │   │   （GatewayClass, Gateway, Secret, ReferenceGrant, Service, Endpoints/EndpointSlice）
    │   ├── wait_kinds_ready(phase1_kinds, 15s)
    │   ├── Phase 2: spawn 依赖资源控制器
    │   │   （HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute, EdgionTls,
    │   │    BackendTLSPolicy, EdgionPlugins, EdgionStreamPlugins, PluginMetaData,
    │   │    LinkSys, EdgionAcme, EdgionGatewayConfig）
    │   └── FileSystemWatcher.run()
    │       ├── init_phase: 扫描目录 → 按 kind 分组 → Init/InitApply/InitDone
    │       └── runtime_phase: notify 监听 → debounce(1s) → Apply/Delete 分发
    ├── 3. wait_registry_ready(30s)
    ├── 4. 触发跨命名空间重新验证
    ├── 5. 创建 ConfigSyncServer + 注册所有 WatchObj
    ├── 6. set_config_sync_server(Some)  ← gRPC 服务可用
    ├── 7. select! { shutdown / reload / error }
    └── 8. 清理：set_config_sync_server(None), abort controller, clear registry
        ├── Shutdown → return
        ├── Reload → continue loop（新 server_id）
        └── Error → 等待 shutdown 信号后 return
```

### Init 阶段细节

`FileSystemWatcher.init_phase()` 执行同步目录扫描：

1. `read_dir()` 遍历配置目录
2. 过滤 `.yaml` / `.yml` 文件，排除 `.status` 文件
3. 读取文件内容，通过 `ResourceKind::from_content()` 识别 Kind
4. 按 Kind 分组后，对每个 Kind 依序发送 `Init` → `InitApply(path, content) * N` → `InitDone`
5. 对有订阅者但无文件的 Kind，仍发送 `Init` → `InitDone`（确保所有控制器完成初始化）

### Runtime 阶段细节

`FileSystemWatcher.runtime_phase()` 使用 `notify` 库的 `RecommendedWatcher`：

1. 创建 `RecommendedWatcher`（poll interval 2s），将原始事件发送到 `mpsc::channel(100)`
2. 忽略 `EventKind::Access`（只读访问不触发处理）
3. 使用 1 秒 interval debounce 去抖：用 `HashMap<PathBuf, EventKind>` 收集路径与**最后一次**事件类型，定时批量分发
   - 同一 debounce 窗口内多个事件时，最新的 EventKind 覆盖旧的（`pending.insert(path, event.kind)`）
4. `dispatch_file_event(path, last_kind)` 通过 `infer_dispatch_action` 三段判定：
   - `Remove(_)` 或 `Modify(Name(From))` → **Delete**（明确移除或改名走，无论文件是否存在）
   - `Create(_)` 或 `Modify(Name(To))` → **Apply**（明确创建或改名进来）
   - 其它（`Modify(Data/Metadata/Any)`、`Any`、`Other`、`Name(Both/Any)`）→ 根据 `path.exists()`：
     - 存在 → Apply
     - 不存在 → **Recheck**：sleep 200ms 后再检查，仍不存在则 Delete
5. 通过 `broadcast::channel` 按 Kind 分发给对应的 `FileSystemResourceController`

### dispatch 设计决策与权衡

**为什么用 EventKind 而不是仅用 path.exists()**

vim、kubectl edit 等工具使用原子替换（`rename(tmp, target)`）写文件。notify 在同一 debounce 窗口内会同时产生 `Remove(target)` 和 `Create(target)` 或 `Modify(Name(Both))`，若恰好在 debounce 边界处仅看到 Remove，旧逻辑的 `!path.exists()` 会触发假 Delete → controller 误删缓存 → 写空 status → 再 Apply。引入 EventKind 后，窗口内 Create 覆盖 Remove，dispatch 看到 Create + exists=true → Apply，根因消除。

**`Remove + path_exists=true → Delete`（保守设计，有意为之）**

Remove 事件明确来自内核，被保守地处理为 Delete，不论 `path.exists()` 的瞬时结果。这样做的原因：
- 正常的 atomic-replace（rename）会让 Create 覆盖 pending 中的 Remove，dispatch 时不会看到 Remove 作为最后事件
- 若 debounce tick 恰好落在 Remove 和 Create 事件之间（毫秒级概率），pending 最后是 Remove，path 可能已重建（exists=true）。此时发出 Delete，下一个 tick 再发 Apply，造成短暂状态抖动——对 config controller 可以接受
- 若改为 `Remove + exists=true → Recheck`，会对每个真实删除多等 200ms，且语义更复杂

**Recheck 200ms 作为事件丢失兜底**

`mpsc::channel(100)` 满时，Remove 事件可能被丢弃，此时 pending 中只剩 `Modify(Data)` + 文件不存在。旧逻辑靠 `!path.exists()` 偶然兜底了这种情况；新逻辑通过 Recheck 显式处理：sleep 200ms 等待可能的 in-flight create 完成，仍不存在则 Delete。

**已知限制**

- Recheck 期间（200ms sleep）select loop 不处理新事件，若 pending 中有多个 Recheck 路径会串行累加延迟。config dir 文件量有限，生产中极少触发。若需高吞吐，可改为 `tokio::spawn` 并发 dispatch。
- init_phase 扫描完成后才启动 watcher，两者之间的事件会丢失（独立已知问题，待单独修复）。

## 分阶段初始化

与 KubernetesCenter 相同，FileSystemCenter 采用两阶段初始化：

| 阶段 | 资源 | 说明 |
|------|------|------|
| Phase 1（基础） | GatewayClass, Gateway, Secret, ReferenceGrant, Service, Endpoints/EndpointSlice | 其他资源在 `parse()` 时需要查询这些基础资源 |
| Phase 2（依赖） | HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, EdgionTls, BackendTLSPolicy, EdgionPlugins, EdgionStreamPlugins, PluginMetaData, LinkSys, EdgionAcme, EdgionGatewayConfig | Phase 1 完成后启动 |

Phase 2 等待 Phase 1 所有 Kind 在 `PROCESSOR_REGISTRY` 中标记 ready（超时 15 秒后降级为并行初始化）。

## FsRawStore 维护

`FileSystemResourceController` 在 `new()` 时创建 `FsRawStore<K>` 并注入到 `processor`，之后在每个事件路径同步维护：

| 事件 / 路径 | FsRawStore 操作 |
|---|---|
| `InitApply` 解析成功 | `upsert(key, Arc::new(obj))` |
| `Apply` 解析成功（init 阶段） | `upsert(key, Arc::new(obj))` |
| `Apply` 解析成功（runtime 阶段，主循环） | `upsert(key, Arc::new(obj))` |
| `Delete` | `remove(&key)` |
| Worker：文件存在且解析成功 | `upsert(key, Arc::new(obj))` — 确保与 worker 实际处理版本一致 |
| Worker：文件不存在 | `remove(&key)` — 兜底 Delete 事件丢失（channel lagged）的情况 |

主循环和 Worker 双重维护是必要的：主循环保证响应及时，Worker 用文件重新读取的最新版本修正可能的版本漂移，并在 channel lagged 时兜底 Delete。

## 与 KubernetesCenter 的差异

| 方面 | FileSystemCenter | KubernetesCenter |
|------|-----------------|-----------------|
| 事件来源 | `notify` 库 inotify/kqueue | K8s API Reflector watch stream |
| Status 持久化 | `.status` 文件（YAML） | K8s API `PATCH /status`（JSON Merge Patch） |
| Leader Election | 无（单实例，`leader_handle = None`） | Lease-based 分布式选举 |
| Status 写入守卫 | `is_none_or` 始终返回 true | `leader_handle.is_leader()` 守卫 |
| 410 Gone 处理 | 不适用 | 触发完整 KubernetesController 重建 |
| 文件解析 | 从文件名提取 Kind/namespace/name | K8s API 返回结构化对象 |
| RawStore 实现 | `FsRawStore`（手动维护 HashMap，双路维护） | `KubeRawStore`（包装 reflector Store，自动同步） |
| Reload | 支持（Admin API 触发，新 server_id） | 支持（同上） |
| 适用场景 | 开发、测试、CI | 生产 K8s 部署 |

## 多版本资源兼容

FileSystemCenter 没有 API Server 帮忙做 CRD 多版本转换，因此多版本兼容应在文件读取边界完成：

- watcher 仍按 `kind` 分发，不按 `apiVersion` 分发
- `ResourceController` 读取 YAML 后，需要先识别 `apiVersion`
- 若命中旧版本资源（例如 `BackendTLSPolicy/v1alpha3`），先反序列化为 legacy 类型
- 然后立即转换为内部 canonical 类型，再写入 `FsRawStore`、进入 workqueue、运行 handler
- worker 出队后重新读文件时，也必须复用同一套“解析并归一化”逻辑

推荐原则：

- 同一个 FileSystem controller 只维护一种内部类型
- 旧版本 YAML 只是输入兼容层
- status 持久化和后续 Gateway sync 都沿用 canonical 类型

## 关键文件

- `src/core/controller/conf_mgr/conf_center/file_system/center.rs` — `FileSystemCenter`（生命周期）
- `src/core/controller/conf_mgr/conf_center/file_system/controller.rs` — `FileSystemController`（spawn 资源控制器）
- `src/core/controller/conf_mgr/conf_center/file_system/storage.rs` — `FileSystemStorage`（CenterApi 实现）
- `src/core/controller/conf_mgr/conf_center/file_system/file_watcher.rs` — `FileSystemWatcher`（文件监听 + 事件分发）
- `src/core/controller/conf_mgr/conf_center/file_system/resource_controller.rs` — `FileSystemResourceController`
- `src/core/controller/conf_mgr/conf_center/file_system/status.rs` — `FileSystemStatusHandler`（.status 文件读写）
- `src/core/controller/conf_mgr/conf_center/file_system/event.rs` — `FileSystemEvent` 枚举
- `src/core/controller/conf_mgr/conf_center/file_system/config.rs` — `FileSystemConfig`
