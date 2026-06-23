# TLSRoute

TLSRoute 根据 TLS ClientHello 中的 SNI（服务器名称指示）将加密的 TLS 连接路由到后端，不在网关处终止 TLS（TLS 透传）。

## 内容

| 文档 | 说明 |
|------|------|
| overview.md | TLSRoute 概念、结构及完整的 SNI 透传示例 |
| backends/ | （目录）如何定义 TLSRoute 转发目标后端及其权重 |
