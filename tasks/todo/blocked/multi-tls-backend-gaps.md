# TODO: 多 TLS 后端支持缺口

> 来源: `tasks/review/ssl/06-incomplete-multi-backend-support.md`

## 现状

项目 TLS 层围绕 BoringSSL 构建。OpenSSL 后端仅有空壳，rustls 被 `#[cfg]` 完全排除。
默认构建使用 BoringSSL，功能完整，**当前无实际影响**。

## OpenSSL 缺口

| 缺失功能 | 影响 |
|---------|------|
| `ssl_ctx_ex_data` | 连接上下文丢失（无 tls_id、匹配信息） |
| `configure_ciphers` | 密码套件配置静默失效 |
| `set_verify_depth` | 验证深度不受控 |
| 运行时证书过期检查 | 过期证书不被主动拒绝 |
| mTLS SAN/CN 白名单回调 | 直接返回错误 |
| InsecureFallback | 语义错误：降级为 PEER 模式，行为与配置意图相反 |

## rustls 缺口

整个 TLS runtime 模块被 `#[cfg(any(boringssl, openssl))]` 排除，无法创建任何 TLS 监听器。
Pingora 0.8.0 新增了 `set_client_cert_verifier()` API，但动态证书加载和 SNI 路由仍被阻塞。
详见 `tasks/todo/rustls-mtls-support-evaluation.md`。

## 优先级

非紧急 — BoringSSL 是默认且唯一生产后端。仅在需要支持 OpenSSL/rustls 部署时启动。
