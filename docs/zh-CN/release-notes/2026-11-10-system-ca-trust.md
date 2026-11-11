# System CA 信任统一为 OS CA 包（破坏性变更）

**发布日期**: 2026-11-10  
**类型**: 破坏性变更  
**影响范围**: Gateway 启动、Gateway 后端 TLS 与所有 Gateway 出向 TLS 客户端、Controller 出向 TLS（etcd 配置中心、OAuth2 凭据源、HTTPDNS 解析）、`edgion-cli` HTTPS

## 概述

`wellKnownCACertificates: System`（以及未配置 CA）现在在所有地方只有一个含义：固定路径的 OS CA 包，
按顺序取第一个存在的文件：

1. `/etc/ssl/certs/ca-certificates.crt`（Debian、Ubuntu、Alpine、官方镜像）
2. `/etc/pki/tls/certs/ca-bundle.crt`（RHEL 系，包括 Rocky 安装包）
3. `/etc/ssl/cert.pem`（macOS）

它没有配置项，`SSL_CERT_FILE` 也不会影响它的选择。自定义信任使用 Kubernetes CA 引用（`caCertificateRefs`），
它会替换 `System`。后端校验还会读取 BoringSSL 的默认证书位置，详见[后端 TLS](../user-guide/http-route/backends/backend-tls.md)。

## 行为变化

**此前：**
- 后端 TLS（`BackendTLSPolicy` `System`）通过 BoringSSL 使用 OS CA 存储。
- Gateway 出向客户端（OIDC/JWKS、LDAP、ext_proc、forward-auth、webhook、LinkSys
  Redis/Kafka/Elasticsearch/OTLP/HTTPDNS、加密健康检查）使用编译进程序的 Mozilla 根证书列表。
- 加入镜像的私有根证书只对后端生效。
- 使用 `System` 信任的 etcd，或者没有 `tls` 块的 `https://` etcd，没有任何根证书，无法验证任何服务器。

**现在：**
- 后端 TLS 与所有出向客户端加载同一个 CA 包文件。加入 OS CA 包（`update-ca-certificates`）的根证书对两边都生效。
- 找不到 CA 包，或者它不是普通文件、超过 8 MiB、PEM 格式错误、没有可用证书、BoringSSL 无法加载时，Gateway 拒绝启动。
- Controller 启动时加载 CA 包并记录日志；找不到时只打 WARN，其使用 `System` 信任的连接会失败。
- `edgion-cli` 用该 CA 包验证 `https://` 服务器，找不到时打印警告。
- 出向信任改为跟随操作系统发行版的根证书计划，而不是编译进程序的 Mozilla 列表。发行版仍然附带的根证书会被信任；
  Mozilla 额外施加的名称约束不再生效。

## 迁移

- 官方镜像和安装包已安装 `ca-certificates`，无需操作。
- 自定义镜像必须安装 `ca-certificates`（或在上述任一路径提供 PEM CA 包）。
- 要信任私有 CA，请在 `BackendTLSPolicy`、资源自身或 `EdgionGatewayConfig.spec.outboundTls.validation`
  中用 `caCertificateRefs` 引用它。

## 相关文档

- [后端 TLS](../user-guide/http-route/backends/backend-tls.md)
- [Gateway 配置：System CA 包](../ops-guide/configuration/gateway-config.md)
- [连接器（LinkSys）](../ops-guide/infrastructure/connectors.md)
