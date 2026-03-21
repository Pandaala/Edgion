---
name: gateway-tls-overview
description: TLS 子系统总览：TLS Store、SNI 匹配、证书管理、BoringSSL/OpenSSL 后端选择。
---

# TLS 子系统总览

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### TLS Store

<!-- TODO:
- tls_store.rs: 主证书存储，按 namespace/name 索引
- cert_matcher.rs: SNI → 证书匹配，支持端口感知
-->

### SNI 匹配流程

<!-- TODO: 客户端 SNI → 查找匹配证书 → 选择最佳匹配 -->

### 证书来源

<!-- TODO:
- Gateway Listener 的 certificateRefs
- EdgionTls 资源
- ACME 自动签发
- Secret 中的证书
-->

### SSL 后端选择

<!-- TODO: BoringSSL (feature: "boringssl") vs OpenSSL (feature: "openssl") -->
