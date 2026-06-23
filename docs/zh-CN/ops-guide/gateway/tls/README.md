# TLS 配置

如何在 Gateway 上终结 TLS、添加高级 TLS 功能，以及自动签发证书。

- [TLS 终结](./tls-termination.md)——标准 Gateway API 监听器终结（`tls.mode: Terminate`）
- [EdgionTls](./edgion-tls.md) 🔌——用于 mTLS、最低 TLS 版本和密码套件控制的扩展
- [ACME 自动证书](./acme.md) 🔌——通过 ACME 协议自动签发和续期证书

如需在不解密的情况下转发加密 TLS 流量，请参见 [TCP / TLS 监听器](../listeners/tcp.md)（Passthrough 模式）和 [TLSRoute 总览](../../../user-guide/tls-route/overview.md)。
