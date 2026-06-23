---
name: 02-gateway
description: edgion-gateway data plane architecture — Pingora integration, route matching, TLS management, plugin system, load balancing, backend discovery, observability, LinkSys.
---

# 02 Gateway Architecture (edgion-gateway)

> edgion-gateway is the data plane, a high-performance proxy server built on Pingora.
> It receives configuration from the Controller via gRPC and processes HTTP/gRPC/TCP/TLS/UDP traffic.

## File list

| File | Topic | Recommended reading scenario |
|------|-------|------------------------------|
| [00-overview.md](00-overview.md) | Gateway overall architecture | First-time understanding of Gateway design |
| [01-startup-shutdown.md](01-startup-shutdown.md) | Startup / shutdown + Pingora integration | Debug startup issues, understand initialization order |
| [02-pingora-lifecycle.md](02-pingora-lifecycle.md) | ProxyHttp callbacks, ConnectionFilter | Modify request handling flow, understand proxy lifecycle |
| [03-routes/SKILL.md](03-routes/SKILL.md) | Route subsystem | Modify route logic, troubleshoot route mismatches |
| [04-tls/SKILL.md](04-tls/SKILL.md) | TLS subsystem | TLS certificate management, mTLS configuration |
| [05-plugin-system.md](05-plugin-system.md) | Plugin system (4-phase execution model) | Understand plugin execution flow, develop new plugins, modify conditional execution logic |
| [dsl-engine.md](dsl-engine.md) | DSL engine architecture (DslPlugin) | Maintain the DSL VM / parser / compiler / bytecode; understand async-sync bridging, panic isolation, DslRuntime lifecycle |
| [dsl-engine-security-model.md](dsl-engine-security-model.md) | DSL sandbox security model — maintainer view | Add or extend a sandbox limit, audit Map/List/string caps, review SSRF/ReDoS posture, locate where each VmLimits field is enforced |
| [06-load-balancing.md](06-load-balancing.md) | Load balancing strategies | Modify LB strategies, understand RoundRobin/EWMA/LeastConn/ConsistentHash selection algorithms |
| [07-backends.md](07-backends.md) | Backend discovery + health check | Modify Endpoint/EndpointSlice discovery, health check configuration, BackendTLSPolicy |
| [08-observe.md](08-observe.md) | Observability | AccessLog zero-copy output, Prometheus metrics, protocol logs |
| [09-link-sys.md](09-link-sys.md) | LinkSys external system integration | ES/Redis/Etcd/Webhook/LocalFile client management, DataSender implementation |
| [10-runtime-store.md](10-runtime-store.md) | Runtime stores | Gateway/Listener configuration storage, ArcSwap lock-free read pattern |
| [11-conf-handler-guidelines.md](11-conf-handler-guidelines.md) | ConfHandler development guidelines | Implement / modify full_set / partial_update for ConfHandler |
| [12-edgion-plugin-dev.md](12-edgion-plugin-dev.md) | HTTP plugin development guide | Develop a new EdgionPlugin |
| [13-stream-plugin-dev.md](13-stream-plugin-dev.md) | Stream plugin development guide | Develop TCP/TLS layer plugins |
| [14-link-sys-dev.md](14-link-sys-dev.md) | LinkSys connector development guide | Develop a new external-system connector |
| [uri-normalization/SKILL.md](uri-normalization/SKILL.md) | URI normalization architecture (full-stack naming table, dual-view, iron rules, security guarantees) | Develop plugins that read/write path, review ACL code, troubleshoot path bypass |

## Architecture overview

```
                          edgion-gateway
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  ConfigSyncClient ──► ClientCache (per-kind) ──► Preparse    │
│                       ├── EventDispatch                      │
│                       └── ConfHandler (per-kind)             │
│                                                              │
│  Pingora Server                                              │
│  ├── ConnectionFilter (TCP layer, StreamPlugins)             │
│  │                                                           │
│  ├── ProxyHttp (HTTP/gRPC lifecycle)                         │
│  │   ├── early_request_filter → timeouts + keepalive         │
│  │   ├── request_filter       → metadata + ACME + route      │
│  │   ├── upstream_peer        → backend selection + LB       │
│  │   ├── upstream_response    → response plugins             │
│  │   └── logging              → AccessLog + Metrics          │
│  │                                                           │
│  ├── TCP/TLS Routes (per-listener routing)                   │
│  └── UDP Routes (per-listener routing)                       │
│                                                              │
│  Admin API (:12001 default)  Probe (:12000)  Metrics (:12090) │
│                                                              │
│  Backends                                                    │
│  ├── Service/EndpointSlice/Endpoint discovery                │
│  ├── Health Check management                                 │
│  └── BackendTLSPolicy                                        │
│                                                              │
│  TLS subsystem                                               │
│  ├── TLS Store (certificate storage + SNI matching)          │
│  ├── Downstream TLS (client→gateway)                         │
│  └── Upstream TLS (gateway→backend mTLS)                     │
│                                                              │
│  LinkSys (ES, Redis, Etcd, Webhook, File)                    │
└──────────────────────────────────────────────────────────────┘
```
