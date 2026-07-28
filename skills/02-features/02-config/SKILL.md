---
name: config-reference
description: Configuration schema reference: complete field definitions for Controller (YAML) and Gateway (YAML) configuration and the EdgionGatewayConfig CRD.
---

# 02 Configuration Reference

> Configuration has three layers — first decide which layer to change, then look up its schema.

## Configuration Layers

| Layer | Carrier | Scope | Change Frequency |
|-------|---------|-------|------------------|
| Controller process configuration | YAML file | A single Controller instance | Low (loaded at startup) |
| Gateway process configuration | YAML file | A single Gateway instance | Low (loaded at startup) |
| Runtime configuration | EdgionGatewayConfig CRD | All Gateways under the same GatewayClass | Medium (takes effect dynamically) |

## File List

| File | Topic |
|------|-------|
| [00-controller-config.md](00-controller-config.md) | Full Controller YAML schema |
| [01-gateway-config.md](01-gateway-config.md) | Full Gateway YAML schema |
| [02-edgion-gateway-config.md](02-edgion-gateway-config.md) | EdgionGatewayConfig CRD schema |
| [03-auth-bootstrap.md](03-auth-bootstrap.md) | Controller and Gateway Admin token bootstrap, 503 fail-close, and token rotation |

## Quick Decision: Which Layer to Change?

| I want to change… | Change this layer |
|-------------------|-------------------|
| Controller gRPC/Admin listen address | Controller YAML `server` |
| conf_center mode (FileSystem/Etcd/Kubernetes) | Controller YAML `conf_center` |
| Controller address that the Gateway connects to | Gateway YAML `gateway` |
| System log level/format | Controller YAML `logging` / Gateway YAML `logging` |
| Access/SSL/TCP/TLS/UDP log toggles and paths | Gateway YAML `*_log` sections |
| Pingora worker thread count/connection pool | Gateway YAML `server` |
| Global HTTP timeouts | EdgionGatewayConfig CRD `spec.httpTimeout` |
| Global retry count | EdgionGatewayConfig CRD `spec.maxRetries` (top-level field; `spec.httpTimeout.backend.defaultMaxRetries` is deprecated) |
| Global real IP extraction | EdgionGatewayConfig CRD `spec.realIp` |
| Global security protection (XFF restriction, SNI match) | EdgionGatewayConfig CRD `spec.securityProtect` |
| Global plugins | EdgionGatewayConfig CRD `spec.globalPluginsRef` |
| Request-body buffering runtime policy | EdgionGatewayConfig CRD `spec.requestBody` |
| Path normalization (reject requests containing `%2F` / invalid percent-encoding) | Gateway process YAML `path_normalization` (restart required) |
