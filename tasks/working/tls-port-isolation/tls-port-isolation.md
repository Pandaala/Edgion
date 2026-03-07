# EdgionTls 端口级证书隔离改造

> 将 `TlsCertMatcher` 从全局 hostname-only 匹配改造为 `(port, hostname)` 二维匹配，
> 与 `GatewayTlsMatcher` 对齐，解决跨端口证书泄漏的安全风险。

## 1. 问题背景

### 改造前架构

```
EdgionTls CRD
    ↓ gRPC sync
ClientCache<EdgionTls>
    ↓ ConfHandler
TlsStore (全局 HashMap<String, TlsEntry>，key = namespace/name)
    ↓ rebuild_matcher_from_data
TlsCertMatcher (全局 HashHost<Vec<Arc<EdgionTls>>>)
    ↓ match_sni(sni)  ← 仅用 hostname，无 port 维度
TlsCallback::load_cert_from_sni()
```

### 安全风险

`TlsCertMatcher` 是一个全局的、仅按 hostname 匹配的证书存储。在
`TlsCallback::load_cert_from_sni()` 中，EdgionTls 查找优先于 Gateway TLS 且不区分端口：

- 如果两个不同的 Gateway 监听不同端口但处理相同 hostname，EdgionTls 的证书会被两者共用
- 一个本应只用于 TLSRoute 的 mTLS 证书可能被 HTTPS listener 错误使用

### 对比：Gateway TLS 已有端口隔离

`GatewayTlsMatcher` 已实现 `(port, hostname)` 二维匹配：

```rust
struct TlsMatcherData {
    port_map: HashMap<u16, HashHost<Vec<GatewayTlsEntry>>>,
    port_catch_all: HashMap<u16, Vec<GatewayTlsEntry>>,
}
```

## 2. 改造目标

1. `TlsCertMatcher` 支持 `(port, hostname)` 二维匹配，与 `GatewayTlsMatcher` 对齐
2. Controller 通过 `parentRef → Gateway → listener → port` 解析端口
3. `TlsCallback::load_cert_from_sni()` 优先使用带 port 的 EdgionTls 查找
4. 向后兼容：无 `parentRefs` 的 EdgionTls 仍可工作（全局 fallback）

## 3. 已完成的改造

### 3.1 EdgionTlsSpec 增加 `resolved_ports` 字段

**文件**: `src/types/resources/edgion_tls.rs`

```rust
#[serde(skip_serializing_if = "Option::is_none", default)]
pub resolved_ports: Option<Vec<u16>>,
```

- Controller 填充的运行时字段，不来自 YAML
- 与 `secret` 字段模式一致（`skip_serializing_if` + `default`）
- `None` = 全局模式（匹配所有端口），`Some([443, 8443])` = 仅匹配指定端口

### 3.2 Controller handler 解析 parentRef → port

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs`

在 `EdgionTlsHandler::parse()` 中新增第 3 步：

```
1. Resolve server Secret  (已有)
2. Resolve CA Secret       (已有)
3. Resolve ports from parentRefs  (新增)
   ├─ parentRef.port → 直接取端口
   └─ parentRef.sectionName → lookup_gateway() → listener.port
```

- 使用已有的 `lookup_gateway()` 函数（`route_utils.rs`）查询 Gateway
- 端口去重、排序后存入 `resolved_ports`
- Gateway 不存在时静默跳过（等 Gateway 到达后通过 requeue 重新处理）

### 3.3 TlsCertMatcher 改造为 (port, hostname) 二维匹配

**文件**: `src/core/gateway/tls/store/cert_matcher.rs`

将单一 matcher 改为双层结构，包裹在同一个 `ArcSwap` 中保证原子更新：

```rust
struct TlsCertMatcherData {
    port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>,
    global_matcher: HashHost<Vec<Arc<EdgionTls>>>,
}

pub struct TlsCertMatcher {
    data: ArcSwap<TlsCertMatcherData>,
}
```

新增方法：
- `match_sni_with_port(port, sni)` — 先查 port-specific，再 fallback 到 global
- `set(port_matcher, global_matcher)` — 原子替换两个 matcher

保留 `match_sni(sni)` 向后兼容（搜索所有 port matcher + global）。

### 3.4 TlsStore 重建 matcher 时按 port 分组

**文件**: `src/core/gateway/tls/store/tls_store.rs`

`rebuild_matcher_from_data()` 改为根据 `resolved_ports` 分流：

- `resolved_ports = Some([443, 8443])` → 注册到 port 443 和 8443 的 matcher
- `resolved_ports = None` → 注册到 global matcher（向后兼容）

### 3.5 TlsCallback 使用带 port 的查找

**文件**: `src/core/gateway/tls/runtime/gateway/tls_pingora.rs`

三处 `match_sni(&sni)` 全部替换为 `match_sni_with_port(self.port, &sni)`：

| 方法 | 改动 |
|------|------|
| `load_cert_from_sni()` | Layer 1 改为 port-aware |
| `build_ssl_log_entry()` | 日志构建改为 port-aware |
| `extract_client_cert_meta()` | mTLS client cert 提取改为 port-aware |

### 3.6 更新导出

**文件**: `src/core/gateway/tls/store/mod.rs`

新增导出 `match_sni_with_port`。

## 4. 改造文件清单

| # | 文件 | 改动类型 | 说明 |
|---|------|---------|------|
| 1 | `src/types/resources/edgion_tls.rs` | 修改 | 增加 `resolved_ports` 字段 + 更新测试构造 |
| 2 | `src/core/controller/.../handlers/edgion_tls.rs` | 修改 | parse() 增加 parentRef → port 解析 |
| 3 | `src/core/gateway/tls/store/cert_matcher.rs` | 重写 | (port, hostname) 二维匹配 + ArcSwap 原子性 |
| 4 | `src/core/gateway/tls/store/tls_store.rs` | 修改 | rebuild 时按 port 分组 |
| 5 | `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | 修改 | 3 处 match_sni → match_sni_with_port |
| 6 | `src/core/gateway/tls/store/mod.rs` | 修改 | 导出 match_sni_with_port |
| 7 | `src/core/gateway/tls/store/conf_handler.rs` | 修改 | 测试代码补 resolved_ports 字段 |
| 8 | `src/core/gateway/tls/validation/cert.rs` | 修改 | 测试代码补 resolved_ports 字段 |

## 5. 向后兼容性

| 场景 | resolved_ports | 行为 |
|------|---------------|------|
| 无 parentRefs | None | 注册到 global matcher，所有端口匹配（与改造前一致） |
| 有 parentRefs 但无 sectionName/port | None | 同上 |
| 有 parentRefs 且有 sectionName | Some([port]) | 仅在对应端口匹配 |
| 有 parentRefs 且有 port 字段 | Some([port]) | 仅在对应端口匹配 |

**查找优先级**：port-specific EdgionTls → global EdgionTls → port-specific Gateway TLS →
global Gateway TLS

## 6. 已知限制

### 6.1 `i32 as u16` 截断

`ParentReference.port`（`i32`）和 `Listener.port`（`i32`）直接 `as u16`。这是项目已有惯例
（`GatewayTlsMatcher` 中同样使用），K8s 端口范围 1-65535 不会越界，但理想情况下应加范围
校验。

### 6.2 `lookup_gateway` 性能

每次调用做全量 JSON 序列化/反序列化 + O(n) 搜索。Gateway 数量通常个位数，`parse()` 不在
热路径，可接受。待 `ProcessorObj` 暴露 `get(key)` 后可优化。

### 6.3 Gateway 更新级联

Gateway listener port 变更后，已解析的 EdgionTls 的 `resolved_ports` 不会自动更新，需等
EdgionTls 本身被 requeue 才会重新解析。可通过 `GatewayRefManager` 机制在后续实现。

## 7. 验证

- `cargo check` 编译通过（0 errors, 0 warnings）
- 81 个 TLS 相关单元测试全部通过
- 26 个 EdgionTls 专项单元测试全部通过
- 0 个 lint 错误

## 8. 待完成

- [ ] Gateway 变更级联 requeue（`GatewayRefManager`）
- [ ] 端口隔离集成测试（验证不同端口的 EdgionTls 证书不交叉匹配）
- [ ] `i32 as u16` 范围校验
