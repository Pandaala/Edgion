# SSL/TLS 审查报告 #07: conf_sync gRPC TLS 配置缺陷

**审查范围**: conf_sync 组件 gRPC mTLS 配置  
**严重等级**: 高  
**审查日期**: 2026-03-30  
**状态**: 待修复

---

## 概述

Edgion 项目中 Controller 与 Gateway 之间的 conf_sync gRPC 通道使用 mTLS 进行身份认证和加密。审查发现该 TLS 实现存在 **4 个子问题**，涉及服务端身份验证缺失、证书热重载缺失、紧急旁路缺少防护、以及证书生成参数固化等方面。这些缺陷组合起来会显著降低生产环境中通信通道的安全性和运维便捷性。

---

## 子问题 1: 客户端 TLS 缺少服务端身份校验（domain_name）

### 问题描述

`load_client_tls()` 在构建 `ClientTlsConfig` 时，仅配置了 CA 根证书和客户端身份（mTLS 双向认证的客户端部分），但**没有调用 `.domain_name()`** 来指定期望的服务端主机名。这意味着 tonic 的 TLS 握手过程中不会校验服务端证书的 CN（Common Name）或 SAN（Subject Alternative Names）是否匹配目标主机。

### 影响文件

- `src/core/gateway/conf_sync/conf_client/grpc_client.rs` 第 287–306 行

### 当前代码

```rust
async fn load_client_tls(cfg: &ConfSyncTlsConfig) -> anyhow::Result<tonic::transport::ClientTlsConfig> {
    cfg.validate()?;
    let cert_path = cfg.cert_path();
    let key_path = cfg.key_path();
    let ca_path = cfg.ca_path();

    let ca = tokio::fs::read(&ca_path).await
        .map_err(|e| anyhow::anyhow!("Failed to read CA cert {:?}: {}", ca_path, e))?;
    let cert = tokio::fs::read(&cert_path).await
        .map_err(|e| anyhow::anyhow!("Failed to read client cert {:?}: {}", cert_path, e))?;
    let key = tokio::fs::read(&key_path).await
        .map_err(|e| anyhow::anyhow!("Failed to read client key {:?}: {}", key_path, e))?;

    Ok(tonic::transport::ClientTlsConfig::new()
        .ca_certificate(tonic::transport::Certificate::from_pem(ca))
        .identity(tonic::transport::Identity::from_pem(cert, key)))
}
```

### 影响分析

| 维度 | 说明 |
|------|------|
| **安全性** | **高风险**。攻击者如果获得同一 CA 签发的任意证书（例如泄露的其他服务证书），即可发起中间人攻击（MITM）。TLS 握手会成功通过，因为证书链验证通过但主机名未校验。 |
| **运维** | 低影响。修复不影响现有部署流程。 |

### 根因

`tonic::transport::ClientTlsConfig` 在不调用 `.domain_name()` 时，底层 `rustls` 默认使用 `Endpoint` URI 中的 host 部分进行 SNI（Server Name Indication）发送，但**不保证**所有版本/配置下都会严格校验证书的 SAN。显式设置 `domain_name` 是防御性编程的必要措施，同时也可以在 URI 使用 IP 地址而证书使用 DNS SAN 时正确工作。

### 修复建议

在 `ConfSyncTlsConfig` 中增加可选的 `server_name` 字段，并在 `load_client_tls` 中调用 `.domain_name()`。

**配置结构变更** (`src/core/common/config/mod.rs`):

```rust
pub struct ConfSyncTlsConfig {
    pub cert: String,
    pub key: String,
    pub ca: String,
    #[serde(default)]
    pub skip_tls: bool,
    /// 期望的服务端证书主机名（CN/SAN），用于 TLS 握手中的身份校验。
    /// 如果未设置，则从 gRPC 连接地址中提取 host 部分作为 domain_name。
    #[serde(default)]
    pub server_name: Option<String>,
}
```

**客户端 TLS 加载变更** (`grpc_client.rs`):

```rust
async fn load_client_tls(
    cfg: &ConfSyncTlsConfig,
    grpc_server_addr: &str,
) -> anyhow::Result<tonic::transport::ClientTlsConfig> {
    cfg.validate()?;
    // ... 文件读取同前 ...

    let domain = cfg.server_name.clone().unwrap_or_else(|| {
        // 从 gRPC 地址中提取 host 部分作为回退
        grpc_server_addr
            .split(':')
            .next()
            .unwrap_or("edgion-controller")
            .to_string()
    });

    Ok(tonic::transport::ClientTlsConfig::new()
        .domain_name(domain)
        .ca_certificate(tonic::transport::Certificate::from_pem(ca))
        .identity(tonic::transport::Identity::from_pem(cert, key)))
}
```

### 优先级: **P0 — 立即修复**

---

## 子问题 2: 证书无热重载机制

### 问题描述

客户端和服务端的 TLS 证书均在启动时一次性从文件系统读取（通过 `tokio::fs::read()`），之后连接/监听器使用的 TLS 配置在整个进程生命周期内保持不变。当证书过期或需要轮换时，必须**完整重启服务**才能加载新证书。

### 影响文件

- `src/core/gateway/conf_sync/conf_client/grpc_client.rs` 第 293–301 行（`load_client_tls`）
- `src/core/controller/conf_sync/conf_server/grpc_server.rs` 第 160–172 行（`load_server_tls`）

### 影响分析

| 维度 | 说明 |
|------|------|
| **安全性** | 中等。无法快速轮换证书意味着被泄露的密钥暴露窗口更大。短有效期证书策略（如 cert-manager 90 天签发）无法充分发挥作用。 |
| **运维** | **高影响**。证书轮换需要滚动重启所有 Gateway 和 Controller 实例。在大规模部署中（数十个 Gateway 实例），这会造成显著的运维复杂度和潜在的流量中断。 |

### 根因

当前架构中，TLS 配置在 `Endpoint`（客户端）和 `Server::builder()`（服务端）创建时绑定，tonic 不原生支持运行时替换 TLS 配置。实现热重载需要额外的抽象层。

### 修复建议

分阶段实施：

**阶段一（短期）: 文件监听 + 客户端重连**

Gateway 侧（客户端）较容易实现，因为 gRPC 客户端可以在检测到证书文件变更后重建 `Channel`：

```rust
use notify::{Watcher, RecursiveMode, Event, EventKind};

async fn watch_cert_files(
    tls_config: ConfSyncTlsConfig,
    endpoint_template: tonic::transport::Endpoint,
    channel_tx: tokio::sync::watch::Sender<Channel>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let _ = tx.blocking_send(());
            }
        }
    }).expect("Failed to create file watcher");

    let paths = [tls_config.cert_path(), tls_config.key_path(), tls_config.ca_path()];
    for p in &paths {
        watcher.watch(p, RecursiveMode::NonRecursive)
            .expect("Failed to watch cert file");
    }

    while rx.recv().await.is_some() {
        // 防抖：等待短暂时间以确保所有文件都已写入完毕
        tokio::time::sleep(Duration::from_secs(2)).await;
        match load_client_tls(&tls_config, /* server_addr */).await {
            Ok(new_tls) => {
                match endpoint_template.clone().tls_config(new_tls) {
                    Ok(ep) => {
                        let new_channel = ep.connect_lazy();
                        let _ = channel_tx.send(new_channel);
                        tracing::info!("TLS certificates reloaded successfully");
                    }
                    Err(e) => tracing::error!("Failed to apply new TLS config: {}", e),
                }
            }
            Err(e) => tracing::error!("Failed to reload TLS certificates: {}", e),
        }
    }
}
```

**阶段二（中期）: 服务端 TLS 重载**

Controller 侧（服务端）需要使用 `tonic` 的 `TlsAcceptor` 替换或利用 `axum`/`hyper` 层面的 TLS 重载机制。可考虑使用 `tokio-rustls` 的 `TlsAcceptor` 配合 `Arc<ServerConfig>` 的原子替换：

```rust
use std::sync::Arc;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

struct ReloadableTlsAcceptor {
    rx: watch::Receiver<Arc<rustls::ServerConfig>>,
}

impl ReloadableTlsAcceptor {
    fn current(&self) -> TlsAcceptor {
        TlsAcceptor::from(self.rx.borrow().clone())
    }
}
```

### 优先级: **P1 — 下一迭代**

---

## 子问题 3: skip_tls 紧急旁路缺乏生产环境防护

### 问题描述

`ConfSyncTlsConfig` 包含一个 `skip_tls: bool` 字段（第 331 行），用于紧急情况下跳过 TLS 以明文启动 gRPC。虽然代码中有 `tracing::error!` 日志警告，但**没有任何机制**防止在生产环境中意外启用此选项。一个错误的配置文件提交或环境变量覆盖即可导致整个 conf_sync 通道降级为明文传输。

### 影响文件

- `src/core/common/config/mod.rs` 第 329–332 行（`skip_tls` 字段定义）
- `src/core/gateway/conf_sync/conf_client/grpc_client.rs` 第 57–61 行（客户端旁路逻辑）
- `src/core/controller/conf_sync/conf_server/grpc_server.rs` 第 74–78 行、第 100–105 行（服务端旁路逻辑）

### 当前代码

```rust
/// Emergency bypass: skip TLS and start in plaintext with ERROR log.
/// Only for emergency use during cert rotation failures. Must not stay enabled.
#[serde(default)]
pub skip_tls: bool,
```

### 影响分析

| 维度 | 说明 |
|------|------|
| **安全性** | **高风险**。`skip_tls` 一旦开启，所有 conf_sync 数据（包含完整的路由配置、服务发现信息、可能的 Secret 引用）均以明文传输。内网中的横向移动攻击者可直接嗅探或篡改配置数据。 |
| **运维** | 中等影响。缺乏自动检测意味着误配置可能长期存在而不被发现。 |

### 根因

`skip_tls` 是一个普通的 `bool` 字段，反序列化时无任何校验。没有环境感知（production vs. development）机制来阻止其在不当环境中启用。

### 修复建议

**方案 A: 环境变量门控（推荐）**

要求同时设置一个特定的环境变量才能使 `skip_tls` 生效：

```rust
impl ConfSyncTlsConfig {
    /// 判断是否真正跳过 TLS，需要 skip_tls=true 且环境变量 EDGION_ALLOW_INSECURE=1
    pub fn effective_skip_tls(&self) -> bool {
        if !self.skip_tls {
            return false;
        }
        match std::env::var("EDGION_ALLOW_INSECURE") {
            Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => {
                tracing::error!(
                    "skip_tls activated with EDGION_ALLOW_INSECURE override. \
                     THIS IS INSECURE. Remove immediately after emergency."
                );
                true
            }
            _ => {
                tracing::error!(
                    "skip_tls=true in config but EDGION_ALLOW_INSECURE not set. \
                     Ignoring skip_tls for safety. Set EDGION_ALLOW_INSECURE=1 if intended."
                );
                false
            }
        }
    }
}
```

**方案 B: 启动时自动过期**

在 `skip_tls` 旁边增加 `skip_tls_until` 时间戳字段，超过该时间后自动失效：

```rust
pub struct ConfSyncTlsConfig {
    // ... 其他字段 ...
    #[serde(default)]
    pub skip_tls: bool,
    /// skip_tls 自动失效时间 (RFC 3339)。超过此时间 skip_tls 自动关闭。
    /// 最大允许窗口：24 小时。
    #[serde(default)]
    pub skip_tls_until: Option<String>,
}
```

**方案 C: 指标暴露**

无论采用哪种方案，都应暴露 Prometheus 指标以便告警：

```rust
// 在 skip_tls 生效时设置 gauge
metrics::gauge!("edgion_conf_sync_tls_disabled").set(1.0);
```

配合告警规则：

```yaml
- alert: ConfSyncTLSDisabled
  expr: edgion_conf_sync_tls_disabled == 1
  for: 5m
  labels:
    severity: critical
  annotations:
    summary: "conf_sync gRPC 通道 TLS 已被禁用"
```

### 优先级: **P1 — 下一迭代**

---

## 子问题 4: 证书生成参数固化，CA 私钥未持久化

### 问题描述

`generate_certs()` 函数存在以下限制：

1. **算法固化**：所有密钥对（CA、Server、Client）均使用 ECDSA P-256 (`PKCS_ECDSA_P256_SHA256`)，无法选择 RSA-2048/4096 或 ECDSA P-384 等其他算法。部分企业安全合规要求使用特定的密钥算法。

2. **默认有效期偏长**：`GenCertsOptions` 中 `cert_days` 默认为 365 天。对于内部 mTLS 通信，行业最佳实践建议使用更短的有效期（90 天或更短），以限制密钥泄露的影响窗口。

3. **CA 私钥未持久化**：`CertBundle` 只保存了 `ca_cert_pem`，CA 的私钥 (`ca_key_pem`) 在函数返回后即丢失（代码中有 TODO 注释）。这意味着后续无法使用同一 CA 签发新证书（例如添加新的 Gateway 实例时），必须重新生成整个证书链并分发到所有节点。

### 影响文件

- `src/core/ctl/certs/mod.rs` 第 1–122 行（完整文件）

### 相关代码

```rust
pub struct CertBundle {
    pub ca_cert_pem: String,
    // TODO: ca_key_pem not stored — Center federation may need it later
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}
```

```rust
let ca_key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
    .context("Failed to generate CA key pair")?;
```

### 影响分析

| 维度 | 说明 |
|------|------|
| **安全性** | 中等。365 天有效期对于内部 mTLS 偏长但不构成直接漏洞。算法固化在多数场景下可接受（P-256 被广泛认可）。 |
| **运维** | **高影响**。CA 私钥丢失是最大的运维痛点——扩容时增加 Gateway 实例需要重新生成全套证书并更新所有现有节点的 CA 证书，这在大规模集群中成本极高。 |

### 根因

`generate_certs()` 是为一次性初始化场景设计的，未考虑持续运维（增量签发、证书轮换）的需求。CA 私钥未存入 `CertBundle` 是一个明确标记了 TODO 但尚未完成的功能。

### 修复建议

**1. 持久化 CA 私钥**

```rust
pub struct CertBundle {
    pub ca_cert_pem: String,
    pub ca_key_pem: String,   // 新增
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

pub fn generate_certs(opts: &GenCertsOptions) -> Result<CertBundle> {
    let ca_key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .context("Failed to generate CA key pair")?;
    let ca_key_pem = ca_key_pair.serialize_pem();

    // ... 其余逻辑不变 ...

    Ok(CertBundle {
        ca_cert_pem,
        ca_key_pem,   // 新增
        server_cert_pem: server_cert.pem(),
        // ...
    })
}
```

同时需要确保 CA 私钥在存储时有严格的文件权限（0600）和访问控制。

**2. 支持可配置的密钥算法**

```rust
#[derive(Clone, Copy)]
pub enum KeyAlgorithm {
    EcdsaP256,
    EcdsaP384,
    // 未来可扩展 RsaXXXX
}

impl KeyAlgorithm {
    fn to_rcgen_alg(&self) -> &'static rcgen::SignatureAlgorithm {
        match self {
            KeyAlgorithm::EcdsaP256 => &rcgen::PKCS_ECDSA_P256_SHA256,
            KeyAlgorithm::EcdsaP384 => &rcgen::PKCS_ECDSA_P384_SHA384,
        }
    }
}

pub struct GenCertsOptions {
    pub ca_days: u32,
    pub cert_days: u32,
    pub server_sans: Vec<String>,
    pub namespace: Option<String>,
    pub key_algorithm: KeyAlgorithm,  // 新增
}
```

**3. 缩短默认有效期**

将 `cert_days` 的默认值从 365 天改为 90 天（与 Let's Encrypt 等最佳实践对齐），CA 有效期可保持 3650 天（10 年）：

```rust
impl Default for GenCertsOptions {
    fn default() -> Self {
        Self {
            ca_days: 3650,
            cert_days: 90,    // 从 365 改为 90
            server_sans: vec![],
            namespace: None,
            key_algorithm: KeyAlgorithm::EcdsaP256,
        }
    }
}
```

**4. 增加增量签发能力**

新增从已有 CA 签发新证书的函数：

```rust
/// 使用已有 CA 签发新的服务端或客户端证书
pub fn sign_cert_with_ca(
    ca_cert_pem: &str,
    ca_key_pem: &str,
    role: CertRole, // Server / Client 枚举
    opts: &SignCertOptions,
) -> Result<(String, String)> { // (cert_pem, key_pem)
    let ca_key_pair = KeyPair::from_pem(ca_key_pem)
        .context("Failed to parse CA key")?;
    let ca_params = CertificateParams::from_ca_cert_pem(ca_cert_pem)
        .context("Failed to parse CA cert")?;
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key_pair);

    // ... 根据 role 生成并签发证书 ...
}
```

### 优先级: **P2 — 近期规划**（CA 私钥持久化部分建议提升至 P1）

---

## 修复优先级汇总

| 优先级 | 子问题 | 描述 | 预估工作量 |
|--------|--------|------|-----------|
| **P0** | #1 | 客户端 TLS 缺少 `domain_name()` 身份校验 | 0.5 天 |
| **P1** | #3 | `skip_tls` 缺少生产环境防护机制 | 1 天 |
| **P1** | #4 (部分) | CA 私钥未持久化（`CertBundle` 补全） | 0.5 天 |
| **P1** | #2 | 证书热重载（客户端侧优先） | 2–3 天 |
| **P2** | #4 (完整) | 证书生成参数可配置化 + 增量签发 | 2 天 |

---

## 附录：文件索引

| 文件路径 | 相关子问题 | 关键行号 |
|----------|-----------|---------|
| `src/core/gateway/conf_sync/conf_client/grpc_client.rs` | #1, #2 | 42, 57–68, 287–306 |
| `src/core/controller/conf_sync/conf_server/grpc_server.rs` | #2 | 69–88, 91–127, 154–173 |
| `src/core/common/config/mod.rs` | #1, #3 | 320–377 |
| `src/core/ctl/certs/mod.rs` | #4 | 13–19, 40–122 |
