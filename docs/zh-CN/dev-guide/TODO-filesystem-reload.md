# TODO: FileSystem Reload 完整重置修复

## 问题描述

当前 FileSystem 模式的 `/api/v1/reload` 端点实现不完整：

### 当前行为（有问题）

```
POST /api/v1/reload
    ↓
load_all_resources(writer, 现有的 config_server)
    ↓
使用 InitAdd 增量添加资源到现有缓存
```

**问题**：
1. 不清空缓存 - 已删除的 YAML 文件对应的资源仍保留在缓存中
2. 不重启 FileWatcher - FileResourceTracker 保持旧状态
3. 不重置 SecretRefManager - 可能导致级联更新逻辑错误

### 期望行为（与 K8s 重选主一致）

```
POST /api/v1/reload
    ↓
1. 停止现有 FileSystemSyncController
    ↓
2. 创建新 ConfigServer（或清空现有缓存）
    ↓
3. 重新启动 FileSystemSyncController
    ↓
4. 完整走 init phase + runtime phase
```

## 对比

| 操作 | K8s 重选主/410 | FileSystem reload (当前) |
|------|---------------|-------------------------|
| 创建新 ConfigServer | ✅ 是 | ❌ 否 (复用) |
| 清空缓存 | ✅ 是 (新实例) | ❌ 否 |
| 删除已移除的资源 | ✅ 是 | ❌ 否 |
| 重启 watcher | ✅ 是 | ❌ 否 |
| 重置 tracker | ✅ 是 | ❌ 否 |
| 完整 init phase | ✅ 是 | ⚠️ 部分 |

## 修复计划

### 1. 在 ConfCenter 添加 reload() 方法

文件：`src/core/conf_mgr/conf_center/mod.rs`

```rust
/// Reload all resources (FileSystem mode only)
/// 
/// Performs a complete reset:
/// 1. Stop existing controller
/// 2. Clear all caches (or create new ConfigServer)
/// 3. Restart FileSystemSyncController with full init + runtime
pub async fn reload(&self) -> Result<()> {
    if self.is_k8s_mode() {
        return Err(anyhow!("Reload not supported in K8s mode"));
    }
    
    // 1. 停止现有控制器
    if let Some(handle) = self.watcher_handle.lock().unwrap().take() {
        handle.abort();
    }
    
    // 2. 获取或创建 ConfigServer
    // 方案 A: 清空现有缓存
    if let Some(cs) = self.config_server() {
        cs.clear_all_caches();
    }
    // 方案 B: 创建新 ConfigServer（更彻底）
    
    // 3. 重新启动 FileSystemSyncController
    let config_server = self.config_server()
        .ok_or(anyhow!("ConfigServer not available"))?;
    self.start_filesystem_sync_controller(&config_server).await?;
    
    // 4. 等待就绪
    self.wait_caches_ready(&config_server, 30).await;
    
    Ok(())
}
```

### 2. 更新 Admin API

文件：`src/core/api/controller/mod.rs`

```rust
async fn reload_all_resources(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<types::ApiResponse<String>>, StatusCode> {
    // 改用新的 reload 方法
    state.conf_center
        .reload()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(types::ApiResponse::success(
        "Resources reloaded successfully".to_string(),
    )))
}
```

### 3. 清理 init_loader.rs

- 标记为 `#[deprecated]` 或删除
- 统一使用 `FileSystemSyncController` 进行资源加载

## 相关文件

- `src/core/conf_mgr/conf_center/mod.rs` - ConfCenter 主模块
- `src/core/conf_mgr/conf_center/lifecycle_filesystem.rs` - FileSystem 生命周期
- `src/core/conf_mgr/conf_center/init_loader.rs` - 当前 reload 使用的加载器（待废弃）
- `src/core/conf_mgr/conf_center/file_system/sync_controller.rs` - 新的同步控制器
- `src/core/api/controller/mod.rs` - Admin API reload 端点
- `src/core/conf_sync/conf_server/config_server.rs` - ConfigServer (有 clear_all_caches 方法)

## 参考

K8s 模式的完整重置实现位于：
- `src/core/conf_mgr/conf_center/lifecycle_kubernetes.rs`
- 每次重选主时调用 `start_event_watchers()` 创建全新的 ConfigServer
