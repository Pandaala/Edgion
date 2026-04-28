---
name: gateway-tls-overview
description: TLS 子系统总览：TLS Store、SNI 匹配、证书管理、BoringSSL/OpenSSL 后端选择。
---

# TLS 子系统总览

## 架构概述

Edgion Gateway 的 TLS 子系统分为两个方向和一个存储层：

```
                    ┌─────────────────────────────┐
                    │         TLS Store            │
                    │  (tls_store + cert_matcher)  │
                    └──────────┬──────────────────┘
                               │
              ┌────────────────┼────────────────┐
              ▼                                 ▼
   ┌──────────────────┐              ┌──────────────────┐
   │  Downstream TLS  │              │   Upstream TLS   │
   │ (客户端→Gateway)  │              │ (Gateway→后端)    │
   │                  │              │                  │
   │ - TLS 终止       │              │ - BackendTLSPolicy│
   │ - SNI 证书选择   │              │ - mTLS 到后端     │
   │ - mTLS 客户端验证│              │ - 证书信息提取    │
   └──────────────────┘              └──────────────────┘
```

## 模块结构

```
src/core/gateway/tls/
├── mod.rs                          # 模块入口，feature flag 条件编译
├── store/
│   ├── mod.rs
│   ├── tls_store.rs                # TlsStore：证书存储与验证
│   ├── cert_matcher.rs             # TlsCertMatcher：listener+SNI 证书匹配
│   └── conf_handler.rs             # ConfHandler<EdgionTls> 实现
├── runtime/
│   ├── mod.rs
│   ├── gateway/
│   │   ├── mod.rs
│   │   └── tls_pingora.rs          # TlsCallback：Pingora listener TLS 回调
│   └── backend/
│       ├── mod.rs
│       ├── backend_api.rs           # 统一后端 API（BoringSSL/OpenSSL 分派）
│       └── cert_extractor.rs        # 客户端证书信息提取
├── validation/
│   ├── mod.rs
│   ├── cert.rs                      # 证书有效性验证
│   └── mtls.rs                      # mTLS 相关验证
├── boringssl/
│   ├── mod.rs
│   └── mtls_verify_callback.rs      # BoringSSL mTLS 验证回调
└── openssl/
    └── mod.rs                       # OpenSSL 预留模块
```

## 证书存储

### TlsStore

`TlsStore` 是证书的规范存储层，以 `namespace/name` 为键存储所有 `EdgionTls` 资源：

- 内部使用 `RwLock<HashMap<String, TlsEntry>>`，每个条目包含 `Arc<EdgionTls>` 和 `CertValidationResult`。
- 证书在写入时立即执行验证（`validate_cert`），无效证书仍然存储但不进入匹配器。
- 写操作（`full_set` / `partial_update`）在持有写锁期间同步重建匹配器，确保存储与匹配器的一致性。
- 全局单例：`GLOBAL_TLS_STORE`（`LazyLock<Arc<TlsStore>>`）。

### TlsCertMatcher

`TlsCertMatcher` 是用于 TLS 握手热路径的高性能匹配器：

```rust
struct TlsCertMatcherData {
    listener_matcher: HashMap<ListenerKey, HashHost<Vec<Arc<EdgionTls>>>>,
}

pub struct TlsCertMatcher {
    data: ArcSwap<TlsCertMatcherData>,
}
```

- 两层索引：`ListenerKey → HashHost(SNI → EdgionTls)`。
- `HashHost` 支持精确和通配主机名匹配（`*.example.com`）。
- 使用 `ArcSwap` 实现无锁读取，匹配调用（`match_sni`）发生在 TLS 握手回调中，必须快速返回。
- 全局单例：`TLS_CERT_MATCHER`（`LazyLock<TlsCertMatcher>`）。
- 仅包含已通过验证且有 `resolved_listeners` 的证书（未绑定 listener 的证书被跳过）。

### GatewayTlsMatcher

`GatewayTlsMatcher` 是 Gateway Listener TLS 配置的匹配器（作为 EdgionTls 匹配失败后的回退层）：

```rust
struct TlsMatcherData {
    listener_map: HashMap<ListenerKey, HashHost<Vec<GatewayTlsEntry>>>,
}
```

- 从 Gateway 资源的 Listener TLS 配置中提取证书引用。
- 同样使用 `ListenerKey → HashHost(hostname → GatewayTlsEntry)` 两层索引。
- `ArcSwap` 无锁读取，内层 `Option` 提供快速路径（无 Gateway TLS 配置时直接返回）。
- 全局单例：`GATEWAY_TLS_MATCHER`。
- 通过 `rebuild_from_gateways` 在 Gateway 配置变更时重建。

## SNI 匹配流程（Phase 1 重构后）

TLS 握手由 BoringSSL 的两个回调按状态机顺序串联：

### 1. 早期回调 `select_certificate_callback`（ClientHello 后、版本协商前）

注册位置：`new_tls_settings_with_callback`（`tls_pingora.rs`）。
触发时机：BoringSSL `handshake_server.cc:694`，**早于** `handshake_server.cc:710` 的版本范围 freeze。

职责（这是 `min_tls_version` 唯一的真实生效点）：

- 从 ClientHello 提取 SNI（`ch.servername(NameType::HOST_NAME)`）
- Layer 1 匹配：`TlsCertMatcher::match_sni(listener_key, sni)` 查 EdgionTls
- 命中且 `et.spec.min_tls_version` 有值 → `ssl.set_min_proto_version(...)` per-connection
  - 失败时 fail-closed：`err_log = "min ver: ..."` + `log_ssl` + 返回 `Err(SelectCertError::ERROR)`
- 把匹配结果（`Arc<EdgionTls>` + 已 resolve 的 SNI）通过 `store_ssl_ctx` 写入 SSL ex_data 作为快照
- 始终返回 `Ok(())`（除版本设置失败外），让握手继续到 cert_cb

为什么不能在 cert_cb 里设置 `min_tls_version`：BoringSSL `handshake_server.cc:710` 注释明确——版本范围在 select_cb 之后立即冻结，cert_cb（`L759`）已晚一步。

### 2. 证书回调 `certificate_callback`（cert_cb，handshake_server.cc:759）

实现 `TlsAccept::certificate_callback`。pingora 用 `SSL_set_cert_cb` 把握手暂停成 async-friendly 形态。

职责：

1. `take_ssl_ctx` 取出 select_cb 写入的快照（消费式取出，自动清空 ex_data 槽）
2. 快照 fast path：`matched_edgion_tls.is_some()` → 直接 `apply_edgion_tls_cert`，**不再 `match_sni`**——单点匹配避免 select_cb / cert_cb 双 match 在 hot-reload 窗口内拿到不同代资源
3. 否则 SNI 解析（优先 snapshot.sni，再 `ssl.servername`，再 `fallback_sni`）
4. 重试 Layer 1（防御性：覆盖非 boringssl 后端 / select_cb store 失败）
5. Layer 2：`GatewayTlsMatcher::match_sni` → 命中 → `apply_gateway_tls_cert`
6. 均未命中 → `err_log = "cert not found"` + `log_ssl`
7. 命中任一后调用 `finalize_ssl_ctx` 盖 `tls_id` + 通过 `store_ssl_ctx` 写回 ex_data，供 `handshake_complete_callback` 取用

`apply_edgion_tls_cert` 在 cert_cb 时机内只做：cert/key 安装、mTLS verify 配置、`configure_ciphers`（TLS1.2 cipher list 在 cert_cb 仍生效，因为 BoringSSL 在 cert_cb 之后才选 cipher——`handshake_server.cc:810`）。**不再处理 `min_tls_version`**。

### 3. 完成回调 `handshake_complete_callback`

`take_ssl_ctx` → 取 client cert info（如 mTLS）→ 盖 `handshake_complete_time` → `log_ssl` → 把 `Arc<TlsConnMeta>` 作为 digest 扩展返回，供 HTTP / TLS proxy 后续读取。

### 4. 早期 abort 审计 hook（Task 10 引入）

当 select_cb 已 store 快照、但 cert_cb 永远不被调用（典型场景：版本协商失败，BoringSSL alert protocol_version 直接终止握手），`free_ssl_ctx` 这个 FFI 释放回调会在 SSL 对象销毁时触发。

`free_ssl_ctx` 检测 `ssl_ctx.meta.tls_id.is_none()` —— 说明 `finalize_ssl_ctx` 没跑——则 push `err_log = "early handshake abort"`（不覆盖更具体的 err_log）+ `log_ssl`。这关闭了"被 min_tls_version 策略拒绝的连接在 ssl.log 无审计记录"的盲区。

包在 `std::panic::catch_unwind` 里防止 panic 跨 FFI 边界（与 `mtls_verify_callback` 同样模式）。

### 整体优先级

```
ClientHello → select_cb (Layer 1 match + min_tls_version + snapshot)
            → freeze version range
            → version negotiation (可能在此 alert 被拒，触发 free_ssl_ctx 审计)
            → cert_cb (consume snapshot OR fallback Layer 1/2 match + apply cert/mtls/ciphers)
            → cipher selection (TLS1.2)
            → handshake_complete_callback (write ssl.log)
```

如均未匹配（Layer 1 + Layer 2）→ ssl.log `"cert not found"`。

## 证书来源

| 来源 | CRD | 匹配层 | 说明 |
|------|-----|--------|------|
| EdgionTls | `edgion.io/EdgionTls` | TlsCertMatcher | Edgion 自有 TLS 资源，支持 mTLS、最低版本、密码套件等高级配置 |
| Gateway Listener | `gateway.networking.k8s.io/Gateway` | GatewayTlsMatcher | 标准 Gateway API TLS 配置，通过 `certificateRefs` 引用 Kubernetes Secret |

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/tls/store/tls_store.rs` | TlsStore：证书存储与验证 |
| `src/core/gateway/tls/store/cert_matcher.rs` | TlsCertMatcher：listener+SNI 匹配 |
| `src/core/gateway/tls/store/conf_handler.rs` | ConfHandler<EdgionTls> 实现 |
| `src/core/gateway/runtime/matching/tls.rs` | GatewayTlsMatcher |
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | TlsCallback |
| `src/core/gateway/tls/validation/cert.rs` | 证书验证 |
