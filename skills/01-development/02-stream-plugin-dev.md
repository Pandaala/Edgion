---
name: stream-plugin-development
description: Stream plugin (EdgionStreamPlugins) development guide. TCP connection-level plugins that run before TLS/HTTP.
---
# Stream Plugin Development

> Stream plugin (EdgionStreamPlugins) development guide. TCP connection-level plugins that run before TLS/HTTP.
>
> **TODO (2026-02-25): P0, New**
> - [ ] `StreamPlugin` trait (`on_connection()` → `StreamPluginResult::Allow/Deny`)
> - [ ] `StreamContext` (client_ip, listener_port, remote_addr)
> - [ ] Directory structure: `src/core/gateway/plugins/stream/`
> - [ ] Checklist: config struct, plugin impl, `EdgionStreamPlugin` enum, StreamPluginStore registration
> - [ ] Gateway listener binding via `edgion.io/edgion-stream-plugins` annotation
> - [ ] Reference implementation: `StreamIpRestriction`
> - [ ] Key differences vs EdgionPlugin: sync vs async, TCP-layer vs HTTP-layer, ConnectionFilter vs ProxyHttp
