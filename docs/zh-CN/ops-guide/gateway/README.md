# Gateway 配置

Gateway 资源的运维配置，包括 GatewayClass、监听器、TLS、重定向和预检策略。

## 内容

| 文档 | 说明 |
|------|------|
| [overview.md](overview.md) | Gateway 资源总览 |
| [gateway-class.md](gateway-class.md) | GatewayClass 配置 |
| [gateway-api-compatibility.md](gateway-api-compatibility.md) | Standard CRD 版本与资源覆盖表 |
| [http-to-https-redirect.md](http-to-https-redirect.md) | HTTP 到 HTTPS 重定向 |
| [preflight-policy.md](preflight-policy.md) | Preflight 请求处理策略 |
| [geoip.md](geoip.md) | GeoIP 地理限制：数据库生命周期、`GeoIpLocation` 插件（HTTP + L4）、访问日志、指标 |
| [listeners/](listeners/README.md) | 监听器配置（HTTP、HTTPS、TCP、gRPC） |
| [tls/](tls/README.md) | TLS 配置（终结、EdgionTls、ACME） |
