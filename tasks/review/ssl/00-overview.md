# Edgion SSL/TLS 代码审查总览

> 审查日期: 2026-03-30  
> 审查范围: `src/core/gateway/tls/`、`src/core/gateway/observe/logs/ssl_log.rs`、`src/core/gateway/runtime/matching/tls.rs`、`src/core/gateway/runtime/server/listener_builder.rs`、`src/core/gateway/backends/gateway_backend_tls_store.rs`、`src/core/common/config/mod.rs`、`src/core/gateway/conf_sync/`、`src/core/controller/conf_sync/`、`src/core/ctl/certs/`、`src/types/resources/edgion_tls.rs`  
> 涉及文件数: ~40+ Rust 源文件

---

## 审查概要

本次审查对 Edgion 项目的 SSL/TLS 子系统进行了全面深入的代码审查，覆盖以下模块：

| 模块 | 路径 | 职责 |
|------|------|------|
| TLS 证书验证 | `tls/validation/` | 证书解析、过期检查、SAN 匹配、mTLS 配置验证 |
| TLS 证书存储 | `tls/store/` | 证书 CRUD、全局存储、SNI 匹配器 |
| TLS 运行时 | `tls/runtime/` | Pingora TLS 回调、证书应用、mTLS 握手 |
| BoringSSL FFI | `tls/boringssl/` | ex_data 管理、自定义验证回调 |
| Gateway TLS 匹配 | `runtime/matching/tls.rs` | Gateway Listener TLS 配置匹配 |
| SSL 日志 | `observe/logs/ssl_log.rs` | TLS 握手日志异步写入 |
| conf_sync TLS | `conf_sync/` + `common/config/` | 控制面-数据面 gRPC mTLS |
| 证书生成 | `ctl/certs/` | CLI 证书链生成 |
| Backend TLS | `backends/` | 上游 TLS 策略存储 |

---

## 问题汇总

共发现 **9 类问题**，按严重程度排序：

### P0 - 严重（安全/正确性）

| # | 问题 | 文件 | 影响 |
|---|------|------|------|
| 01 | [证书私钥匹配未实现](./01-cert-key-matching-not-implemented.md) | `validation/cert.rs` | 错配的 cert/key 通过验证，运行时握手失败 |
| 03 | [BoringSSL FFI 不安全代码](./03-unsafe-ffi-boringssl-callbacks.md) | `tls_pingora.rs`, `mtls_verify_callback.rs` | UB 风险、指针转换依赖非公开布局、transmute |
| 07-1 | [conf_sync 客户端无服务端身份验证](./07-conf-sync-tls-gaps.md) | `grpc_client.rs` | MITM 攻击风险 |

### P1 - 高（功能/可靠性）

| # | 问题 | 文件 | 影响 |
|---|------|------|------|
| 02 | [通配符匹配逻辑重复且不一致](./02-duplicated-wildcard-matching.md) | 4 个文件 | 大小写处理不一致导致安全语义偏差 |
| 04 | [RwLock 中毒处理不一致](./04-rwlock-poisoning-inconsistency.md) | `tls_store.rs` | 写端恢复但读端 panic，级联故障 |
| 06 | [多后端支持不完整](./06-incomplete-multi-backend-support.md) | 多个文件 | OpenSSL/rustls feature 几乎不可用 |
| 09 | [证书验证缺陷（链+IP SAN）](./09-cert-validation-gaps.md) | `validation/cert.rs` | 证书链被忽略、IP SAN 不支持 |

### P2 - 中（效率/可维护性）

| # | 问题 | 文件 | 影响 |
|---|------|------|------|
| 05 | [SSL 日志无界 Channel](./05-ssl-log-unbounded-channel.md) | `ssl_log.rs` | 高负载下 OOM 风险 |
| 07-2 | [conf_sync 证书无热重载](./07-conf-sync-tls-gaps.md) | `grpc_client.rs`, `grpc_server.rs` | 证书轮换需重启 |
| 08 | [静默失败与低效](./08-silent-failures-and-inefficiencies.md) | 多个文件 | Cipher 配置失败不报错、证书双重解析、tls_id 碰撞 |

---

## 关键发现

### 1. 安全方面
- **证书私钥匹配验证完全缺失**，`check_key_valid()` 只做 PEM 解析，不验证公钥匹配
- **mTLS 白名单匹配大小写敏感**，但 TLS 标准要求域名匹配不区分大小写（RFC 6125）
- **conf_sync gRPC 客户端不验证服务端身份**，缺少 `domain_name()` 配置
- **InsecureFallback 验证回调**无条件返回 1，完全绕过证书验证

### 2. 可靠性方面
- **TlsStore 读操作在 lock 中毒时 panic**，一个写线程异常将导致整个 HTTPS 网关不可用
- **SSL 日志使用无界 channel**，高并发下可能 OOM
- **GatewayBackendTlsStore** 的 set/remove 操作存在 TOCTOU 竞态

### 3. 兼容性方面
- **OpenSSL 后端**几乎是空壳，仅有 3 行代码
- **rustls 后端**被 TLS 运行时模块完全排除，无法创建 TLS 监听器
- **非 BoringSSL** 下 TLS 连接上下文（tls_id、匹配信息、客户端证书）全部丢失

### 4. 代码质量
- **通配符匹配逻辑重复 4 处**，行为不一致
- **证书在验证流程中被解析两次**
- **`CertValidationError::KeyMismatch`** 已定义但从未使用（死代码）

---

## 修复优先级建议

### 立即修复（Sprint 1）
1. **01 - 实现 cert/key 匹配验证** — 利用 `boring::PKey::public_eq()` 实现，改动最小
2. **07-1 - 添加 `domain_name()` 配置** — 一行代码修复 MITM 风险
3. **06-partial - 非 BoringSSL InsecureFallback 语义修复** — 行为与配置意图完全相反

### 短期修复（Sprint 2）
4. **02 - 统一通配符匹配** — 提取共享 `host_match.rs` 模块
5. **04 - TlsStore 迁移 `parking_lot::RwLock`** — 已有依赖，零新增成本
6. **09 - 补充 IP SAN 支持** — 参考 `cert_extractor.rs` 已有实现
7. **08-1 - Cipher 配置返回真实错误** — 改 `Ok(())` 为 `Err(...)`

### 中期修复（Sprint 3-4）
8. **03 - FFI 安全重构** — 引入集中的 `ssl_ffi.rs` + 编译时断言
9. **05 - SSL 日志有界 channel** — 替换为 bounded + try_send + metrics
10. **09-2 - 证书链验证** — 遍历所有 PEM 块，验证中间证书
11. **07-2 - conf_sync 证书热重载** — 文件监听 + Channel 重建

### 长期改进（后续迭代）
12. **06-full - OpenSSL/rustls 完整支持** — 需要 TLS 后端抽象层重构
13. **08-3 - tls_id 改用 UUID v7** — 消除碰撞风险
14. **07-4 - skip_tls 环境变量门控** — 防止误开

---

## 涉及的核心文件清单

```
src/core/gateway/tls/
├── mod.rs
├── validation/
│   ├── cert.rs            ← #01, #09
│   ├── mtls.rs            ← #02
│   ├── backend_san.rs     ← #02
│   └── mod.rs
├── store/
│   ├── tls_store.rs       ← #04
│   ├── cert_matcher.rs
│   ├── conf_handler.rs
│   └── mod.rs
├── runtime/
│   ├── gateway/
│   │   ├── tls_pingora.rs ← #03, #06, #08
│   │   └── mod.rs
│   ├── backend/
│   │   ├── backend_api.rs ← #06
│   │   ├── cert_extractor.rs
│   │   └── mod.rs
│   └── mod.rs
├── boringssl/
│   ├── mtls_verify_callback.rs ← #03
│   └── mod.rs
└── openssl/
    └── mod.rs             ← #06

src/core/gateway/observe/logs/
├── ssl_log.rs             ← #05

src/core/gateway/runtime/
├── matching/tls.rs
├── server/listener_builder.rs ← #06

src/core/gateway/backends/
├── gateway_backend_tls_store.rs ← #08

src/core/common/config/
├── mod.rs                 ← #07

src/core/gateway/conf_sync/conf_client/
├── grpc_client.rs         ← #07

src/core/controller/conf_sync/conf_server/
├── grpc_server.rs         ← #07

src/core/ctl/certs/
├── mod.rs                 ← #07

src/types/resources/
├── edgion_tls.rs          ← #02
```

---

## 统计

| 指标 | 数值 |
|------|------|
| 审查文件数 | 40+ |
| 发现问题数 | 9 类（含多个子问题） |
| P0（严重）| 3 |
| P1（高）| 4 |
| P2（中）| 3+ |
| 涉及 unsafe 代码块 | 12+ |
| 重复代码区域 | 4 处通配符匹配 |
| 死代码 | `KeyMismatch` 枚举变体 |
