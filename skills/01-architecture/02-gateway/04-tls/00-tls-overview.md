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
│   ├── cert_matcher.rs             # TlsCertMatcher：port+SNI 证书匹配
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
    port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>,
}

pub struct TlsCertMatcher {
    data: ArcSwap<TlsCertMatcherData>,
}
```

- 两层索引：`Port → HashHost(SNI → EdgionTls)`。
- `HashHost` 支持精确和通配主机名匹配（`*.example.com`）。
- 使用 `ArcSwap` 实现无锁读取，匹配调用（`match_sni_with_port`）发生在 TLS 握手回调中，必须快速返回。
- 全局单例：`TLS_CERT_MATCHER`（`LazyLock<TlsCertMatcher>`）。
- 仅包含已通过验证且有 `resolved_ports` 的证书（未绑定端口的证书被跳过）。

### GatewayTlsMatcher

`GatewayTlsMatcher` 是 Gateway Listener TLS 配置的匹配器（作为 EdgionTls 匹配失败后的回退层）：

```rust
struct TlsMatcherData {
    port_map: HashMap<u16, HashHost<Vec<GatewayTlsEntry>>>,
}
```

- 从 Gateway 资源的 Listener TLS 配置中提取证书引用。
- 同样使用 `Port → HashHost(hostname → GatewayTlsEntry)` 两层索引。
- `ArcSwap` 无锁读取，内层 `Option` 提供快速路径（无 Gateway TLS 配置时直接返回）。
- 全局单例：`GATEWAY_TLS_MATCHER`。
- 通过 `rebuild_from_gateways` 在 Gateway 配置变更时重建。

## SNI 匹配流程

TLS 握手时，`TlsCallback::match_and_apply_cert` 按以下优先级查找证书：

```
1. EdgionTls（TlsCertMatcher::match_sni_with_port）
   └── 匹配成功 → 应用 EdgionTls 证书
       ├── 配置 mTLS（如有 client_auth）
       ├── 设置最低 TLS 版本（如有 min_tls_version）
       └── 设置密码套件（如有 ciphers）

2. Gateway TLS（GatewayTlsMatcher::match_sni_with_port）
   └── 匹配成功 → 从 Gateway Listener 的 certificateRefs 获取 Secret
       ├── 优先使用内联 secrets
       └── 回退到从全局 SecretStore 查找

3. 均未匹配 → 记录 "cert not found" 错误日志
```

## 证书来源

| 来源 | CRD | 匹配层 | 说明 |
|------|-----|--------|------|
| EdgionTls | `edgion.io/EdgionTls` | TlsCertMatcher | Edgion 自有 TLS 资源，支持 mTLS、最低版本、密码套件等高级配置 |
| Gateway Listener | `gateway.networking.k8s.io/Gateway` | GatewayTlsMatcher | 标准 Gateway API TLS 配置，通过 `certificateRefs` 引用 Kubernetes Secret |

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/tls/store/tls_store.rs` | TlsStore：证书存储与验证 |
| `src/core/gateway/tls/store/cert_matcher.rs` | TlsCertMatcher：port+SNI 匹配 |
| `src/core/gateway/tls/store/conf_handler.rs` | ConfHandler<EdgionTls> 实现 |
| `src/core/gateway/runtime/matching/tls.rs` | GatewayTlsMatcher |
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | TlsCallback |
| `src/core/gateway/tls/validation/cert.rs` | 证书验证 |
