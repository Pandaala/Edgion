# Gateway Configuration

Operational configuration for Gateway resources, including GatewayClass, listeners, TLS, redirects, and preflight policies.

## Contents

| Document | Description |
|----------|-------------|
| [overview.md](overview.md) | Gateway resource overview |
| [gateway-class.md](gateway-class.md) | GatewayClass configuration |
| [gateway-api-compatibility.md](gateway-api-compatibility.md) | Standard CRD release and resource coverage matrix |
| [http-to-https-redirect.md](http-to-https-redirect.md) | HTTP to HTTPS redirect |
| [preflight-policy.md](preflight-policy.md) | Preflight request handling policy |
| [geoip.md](geoip.md) | GeoIP restriction: database lifecycle, the `GeoIpLocation` plugin (HTTP + L4), access log, metrics |
| [listeners/](listeners/README.md) | Listener configuration (HTTP, HTTPS, TCP, gRPC) |
| [tls/](tls/README.md) | TLS configuration (termination, EdgionTls, ACME) |
