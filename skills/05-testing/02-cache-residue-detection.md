---
name: 02-cache-residue-detection
description: Cache residue detection — wipe-all + store-stats based leak detection at end of integration runs. Use when adding new in-memory stores, debugging leak reports, or extending the validator.
---

# Cache Residue Detection

> 在 `run_integration.sh` 全部 scenario 跑完之后，调 controller `wipe-all` 清空全部资源，然后断言所有 in-memory cache 计数为 0。任何残留视为 **leak**。

## 工作流

1. `run_integration.sh` 在所有 test loop 结束后、`cleanup` 之前调 `cache_residue` 二进制。
2. `cache_residue` 调 `POST controller /api/v1/admin/wipe-all`（仅 testing-mode 注册），按 tier 顺序删除全部 21 个 kind。
3. 调 `DELETE gateway /api/v1/testing/access-logs` 清 access log（去除 TTL false positive）。
4. 轮询（默认 500ms 间隔，最多 30s）：
   - `GET controller /api/v1/debug/store-stats`
   - `GET gateway /api/v1/debug/store-stats`
   - 内置 ignore-path 过滤结构性常量字段（如 `next_resource_version`）。
5. 全 0 → exit 0；超时仍有残留 → exit 1（mode=fail）/ 0 + warning（mode=warn）；端点不可达 → exit 2。

## 添加新的 store 到 leak 检测

新增任何 in-memory store / cache 时，必须接入 store-stats，否则 `cache_residue` 检测不到该 store 的 leak。

### Gateway 端

1. 在你的 store 上加 `pub fn count(&self) -> usize` 和 `pub fn keys(&self) -> Vec<String>`。
2. 在 `src/core/gateway/api/mod.rs` 的 `StoreStatsResponse` 加字段，并在 `store_stats()` handler 里填充。
3. 加单测：`empty store reports zero`。

### Controller 端

1. 在你的 store 上加同样的方法。
2. 在 `src/core/controller/api/debug_handlers.rs` 的 `ControllerStoreStats` 加字段并填充。
3. 加单测。

### 何时**不**需要加

- Store 与某个父对象同生命周期（例如 plugin instance 内部 cache、route handler 私有 map）：父对象被 ArcSwap drop 时整体回收，无独立 leak 信号。leak 由父对象的 store-stats 暴露即可。
- TTL-based store（如 access log）：在 cache_residue 调用前已有 clear API。

## 添加新的 ignore-path

ignore-path 是结构性常量（不会归零的合法值），如：
- 单调递增的 sequence counter
- 固定数量的注册项（kind 列表、watch object 数）
- 测试期 `>0` 是正常的（如 GatewayConfigStore.gateways）

修改 `IGNORE_PATHS` 常量于 `examples/code/validator/cache_residue.rs`。每加一项必须附 commit message 说明为什么这个字段不算 leak。

## 反向验证

任何对 store-stats 或 wipe-all 的扩展，必须人工跑反向测试：临时短路 delete 路径 → 跑 `run_integration.sh` → 验证 cache_residue 报告精准定位。详见 spec §6.2。

## 调试 leak 报告

报告示例：

```
[FAIL] Cache residue detected after wipe-all (timeout 30s):

  controller:
    conf_center_storage.HTTPRoute        = 1
    secret_store                          = 2
  gateway:
    client_caches.by_kind.Service         = 1   sample: ["default/svc-x"]
    plugin_store_v2.by_kind.JwtAuth       = 1   sample: ["default/jwt-a"]

Total residue: 5 entries across 4 buckets
```

诊断顺序：
1. `controller.conf_center_storage.<Kind>` 残留 → wipe-all 漏删该 kind，检查 `wipe_handlers.rs` 的 TIER 列表
2. `gateway.client_caches.by_kind.<Kind>` 残留 → controller→gateway 同步未对齐（增加 `--timeout-secs`），或 gateway ConfigClient delete 路径有 bug
3. `gateway.<store_name>.*` 残留 → 该 store 的 delete 处理器没正确移除条目；按 sample key 倒查代码
4. `gateway.plugin_store_v2.by_kind.<Kind>` 残留 → PluginStore 的 conf_handler 未响应 delete 事件

## 相关链接

- Spec: `docs/superpowers/specs/2026-04-26-cache-residue-detection-design.md`
- Plan: `docs/superpowers/plans/2026-04-26-cache-residue-detection.md`
- Validator: `examples/code/validator/cache_residue.rs`
- Wipe handler: `src/core/controller/api/wipe_handlers.rs`
- Gateway store-stats: `src/core/gateway/api/mod.rs:312-442`
- Controller store-stats: `src/core/controller/api/debug_handlers.rs`
