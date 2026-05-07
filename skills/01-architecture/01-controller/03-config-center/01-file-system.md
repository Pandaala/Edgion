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
├── config: FileSystemConfig        # 配置（conf_dir 等）
├── config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>
├── reload_tx: Mutex<Option<mpsc::Sender<()>>>
└── controller_handle: Mutex<Option<JoinHandle<()>>>

FileSystemController.run()
├── FileSystemWatcher（共享，集中式文件监听）
│   ├── [PollWatcher (1s stat-diff)] → [dispatcher] → [kind channels]
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
    │   │   （GatewayClass, Gateway, Secret, ReferenceGrant, Service, EndpointSlice）
    │   ├── wait_kinds_ready(phase1_kinds, 15s)
    │   ├── Phase 2: spawn 依赖资源控制器
    │   │   （HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute, EdgionTls,
    │   │    BackendTLSPolicy, EdgionPlugins, EdgionStreamPlugins, PluginMetaData,
    │   │    LinkSys, EdgionAcme, EdgionGatewayConfig）
    │   └── FileSystemWatcher.run()
    │       ├── init_phase: 扫描目录 → 按 kind 分组 → Init/InitApply/InitDone
    │       └── runtime_phase: PollWatcher（1s stat 差分） → Apply/Delete 直接分发
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

`FileSystemWatcher.runtime_phase()` 使用 `notify` 库的 `PollWatcher`：

1. 创建 `PollWatcher`（poll_interval = 1s），将原始事件发送到 `mpsc::unbounded_channel`
2. 忽略 `EventKind::Access`（只读访问不触发处理）
3. 主循环对每个事件直接调用 `dispatch_file_event`，不做应用层 debounce ——
   PollWatcher 已在每个 poll 周期内对同一路径合并到一个事件
4. `dispatch_file_event(path, kind)` 通过 `infer_dispatch_action` 判定：
   - `Remove(_)` 或 `Modify(Name(From))` → **Delete**
   - `Create(_)` 或 `Modify(Name(To))` → **Apply**
   - 其它模糊事件 → 根据 `path.exists()` 判定 Apply / Recheck（PollWatcher 几乎不发模糊事件，此分支保留作 defense-in-depth）
5. 通过 `broadcast::channel` 按 Kind 分发给对应的 `FileSystemResourceController`

### 设计决策与权衡

**为什么用 PollWatcher 而不是 RecommendedWatcher（FSEvents/inotify）**

最初实现使用 `RecommendedWatcher`（macOS 上是 FSEvents、Linux 上是 inotify），并在应用层加了 1 秒 debounce + `HashMap<PathBuf, EventKind>` 收集 + `had_genuine_remove` 跟踪等机制处理事件去重和 atomic-replace 语义。

**实测发现**：macOS FSEvents 在高并发批量删除场景下会**静默丢失大量事件**——一次删除 656 个文件（156ms 内完成），FSEvents 只投递了约 64 个 Remove 事件（损失率 ~90%），且未设置 `kFSEventStreamEventFlagMustScanSubDirs` rescan 标志，OS 完全没有溢出告警。这导致 `cache_residue` 集成测试持续失败，runtime store 与 gateway client cache 都无法清空。

切换到 `PollWatcher` 后：
- 永不丢事件（每个 poll 周期 stat 整个目录，对比上次快照差分）
- 应用层不再需要 debounce / pending HashMap / `had_genuine_remove`，代码简化约 100 行
- 跨平台行为一致（PollWatcher 完全是 user-space 实现，与 OS push API 解耦）

**代价**：固定 1s 延迟（poll_interval）。对 config controller 完全可接受。

**FS 模式延迟下限**

FileSystemCenter 模式下，从「写入配置文件」到「Apply 事件分发」的最小延迟为 1 秒（PollWatcher 的下一个 poll tick）。这是 PollWatcher 设计本身的属性，不是 bug。

测试和工具如果对 FS 模式的响应做了断言（例如「写文件后 < N 秒内状态应更新」），需确保超时预算 ≥ 1.5 秒。

**已知限制**

- **删后立刻重建（同 1s 周期内）会被合并为单个 Modify**：PollWatcher 只看快照差分，看不到中间删除态。如果 ConfHandler 依赖在 on_delete / on_change 之间做差量计算（例如反向引用清理），需要在 `parse()` 里基于「旧值 vs 新值」做对比，而不是依赖 on_delete。详见 `reference_grant.rs` 与 `backend_tls_policy.rs` 的实现模式。
- init_phase 扫描完成后才启动 watcher，两者之间的事件会丢失（独立已知问题，待单独修复）。

## 分阶段初始化

与 KubernetesCenter 相同，FileSystemCenter 采用两阶段初始化：

| 阶段 | 资源 | 说明 |
|------|------|------|
| Phase 1（基础） | GatewayClass, Gateway, Secret, ReferenceGrant, Service, EndpointSlice | 其他资源在 `parse()` 时需要查询这些基础资源 |
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
| 事件来源 | `notify::PollWatcher`（1s stat-diff） | K8s API Reflector watch stream |
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
