---
name: features
description: Use when looking up configuration schemas, resource field definitions, binary deployment options, observability settings, or edgion.io/* annotation references.
---

# 02 Features and Configuration Reference

> Deep documentation of each Edgion feature module, written for **users and operators**.
> See `../01-architecture/SKILL.md` for architecture implementation details, and `../03-coding/SKILL.md` for coding conventions.

## Design Principles

- **Schema-driven**: every resource and every configuration has a complete field definition and type description
- **Gateway API v1.5 aligned**: Gateway API standard resources follow the v1.5.0 specification, with Edgion extension points called out
- **Aligned with architecture numbering**: `03-resources/` numbering corresponds one-to-one with `01-architecture/05-resources/`
- **User perspective**: focuses on "how to use" and "how to configure", not "how it is implemented internally"

## Directory Overview

| # | Directory | Purpose |
|---|-----------|---------|
| 01 | [binary-and-deployment/](01-binary-and-deployment/SKILL.md) | Startup of the four binaries, CLI arguments, deployment patterns, Feature Flags |
| 02 | [config/](02-config/SKILL.md) | Controller (YAML) and Gateway (YAML) configuration schemas, EdgionGatewayConfig CRD schema |
| 03 | [resources/](03-resources/SKILL.md) | **All resource feature schemas**: Gateway, Route, TLS, Plugin, Backend, LinkSys |
| 04 | [observability/](04-observability/SKILL.md) | Observability features: Access Log configuration, Metrics endpoints, protocol logs |
| 05 | [annotations/](05-annotations/SKILL.md) | Annotations and Options reference: complete list of all `edgion.io/*` keys |

## Quick Lookup

| You want to… | Start here |
|--------------|-----------|
| Learn how to start Controller/Gateway | [01-binary-and-deployment/](01-binary-and-deployment/SKILL.md) |
| Modify YAML configuration files | [02-config/](02-config/SKILL.md) |
| Configure a Gateway Listener | [03-resources/01-gateway.md](03-resources/01-gateway.md) |
| Configure HTTP routing rules | [03-resources/04-httproute.md](03-resources/04-httproute.md) |
| Configure gRPC routing | [03-resources/05-grpcroute.md](03-resources/05-grpcroute.md) |
| Configure TLS certificates/mTLS | [03-resources/09-edgion-tls.md](03-resources/09-edgion-tls.md) |
| Use automatic certificates (ACME) | [03-resources/17-acme.md](03-resources/17-acme.md) |
| Look up the configuration of a plugin | [03-resources/13-edgion-plugins.md](03-resources/13-edgion-plugins.md) |
| Configure health checks/load balancing | [03-resources/16-service-backends.md](03-resources/16-service-backends.md) |
| Configure Access Log output | [04-observability/00-logging.md](04-observability/00-logging.md) |
| Integrate with Redis/ES and other external systems | [03-resources/18-link-sys.md](03-resources/18-link-sys.md) |
| Look up the meaning of an annotation | [05-annotations/](05-annotations/SKILL.md) |

## Gateway API Version

Edgion is based on **Gateway API v1.5.0**, with the following support scope:

| Resource | API Version | Support Status |
|----------|-------------|----------------|
| Gateway | `gateway.networking.k8s.io/v1` | Core |
| GatewayClass | `gateway.networking.k8s.io/v1` | Core |
| HTTPRoute | `gateway.networking.k8s.io/v1` | Core |
| GRPCRoute | `gateway.networking.k8s.io/v1` | Core |
| ReferenceGrant | `gateway.networking.k8s.io/v1` (v1beta1 compatible) | Core |
| TCPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |
| TLSRoute | `gateway.networking.k8s.io/v1` | Standard (compatibility with the older alpha version depends on the install manifest) |
| UDPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |
| BackendTLSPolicy | `gateway.networking.k8s.io/v1` (`v1alpha3` compatible) | Standard / multi-version compatible |

Edgion extension resources (API group: `edgion.io`):

| Resource | API Version | Purpose |
|----------|-------------|---------|
| EdgionGatewayConfig | `edgion.io/v1alpha1` | GatewayClass-level runtime configuration |
| EdgionTls | `edgion.io/v1` | Extended TLS configuration (mTLS, versions, cipher suites) |
| EdgionPlugins | `edgion.io/v1` | HTTP plugin configuration |
| EdgionStreamPlugins | `edgion.io/v1` | TCP/TLS layer plugin configuration |
| EdgionAcme | `edgion.io/v1` | ACME automatic certificate management |
| LinkSys | `edgion.io/v1` | External system connector |
| EdgionConfigData | `edgion.io/v1` | Plugin metadata |
