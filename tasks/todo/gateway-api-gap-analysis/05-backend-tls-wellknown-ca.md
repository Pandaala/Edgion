# BackendTLSPolicy wellKnownCACertificates 实现

## 优先级: P2
## 工作量: 小（预计 1 天）
## 类型: 功能缺失

---

## 问题描述

BackendTLSPolicy 的 `validation.wellKnownCACertificates` 字段已定义（枚举值 `System`），
但从未在运行时生效。当用户不想手动提供 CA 证书而希望使用系统 CA 信任链时，此功能不可用。

Gateway API 规范要求：`caCertificateRefs` 和 `wellKnownCACertificates` 二选一。
目前只有 `caCertificateRefs` 路径可用。

## 当前状态

### 已有
- `WellKnownCACertificates` 枚举定义（`System` 值）
- `BackendTLSPolicyValidation.well_known_ca_certificates: Option<WellKnownCACertificates>`
- 字段可正确序列化/反序列化

### 缺失
- Controller 未处理 `wellKnownCACertificates = System` 的情况
- Gateway 建立后端 TLS 连接时未加载系统 CA 证书
- 未实现 `caCertificateRefs` 和 `wellKnownCACertificates` 的互斥校验

## 涉及文件

1. `src/types/resources/backend_tls_policy.rs` — 类型定义
2. `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs` — Controller 处理
3. `src/core/gateway/tls/runtime/backend/` — 后端 TLS 连接配置
4. 平台 CA 路径：macOS `/etc/ssl/cert.pem`，Linux `/etc/ssl/certs/ca-certificates.crt` 等

## 实现建议

1. Controller 校验：`caCertificateRefs` 和 `wellKnownCACertificates` 不能同时设置
2. 当 `wellKnownCACertificates = System` 时：
   - 使用 `rustls-native-certs` crate（如用 rustls）或 OpenSSL/BoringSSL 的 `SSL_CTX_set_default_verify_paths()`
   - 加载操作系统默认 CA 信任链
3. 在 resolved 数据中标记 "使用系统 CA"，Gateway 据此配置 SSL context
4. 如果系统 CA 加载失败，设置 ResolvedRefs=False 并报告原因

### BoringSSL/OpenSSL 路径（默认构建）
```rust
// 使用 SSL_CTX_set_default_verify_paths() 加载系统默认 CA
ssl_ctx.set_default_verify_paths()?;
```

### Rustls 路径（feature flag）
```rust
// 使用 rustls-native-certs
let certs = rustls_native_certs::load_native_certs()?;
```

## 参考 Skills

- `skills/01-architecture/02-gateway/SKILL.md` — Gateway TLS 架构
- `skills/09-misc/debugging-tls-gateway.md` — TLS 调试

## 验证标准

- 配置 `wellKnownCACertificates: System` 时，后端 TLS 使用系统 CA 验证
- 同时配置 `caCertificateRefs` 和 `wellKnownCACertificates` 时报错（互斥）
- 两者都未配置时报错
- 系统 CA 加载失败时 ResolvedRefs=False
- `cargo check --all-targets` 通过
- 集成测试验证系统 CA 路径
