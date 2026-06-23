---
name: webhook-extract-ready-ips-ipv6-brackets
description: Read when reviewing LinkSys webhook-service endpoint findings about extract_ready_ips building {ip}:{port} without IPv6 brackets / malformed IPv6 webhook backend URLs.
---

> **OBSOLETE (2026-06-20)** — The controller-side webhook endpoint injection path described in
> this document has been entirely removed by the 2026-06-20 endpoint-decouple refactor
> (`docs/superpowers/specs/2026-06-20-linksys-webhook-endpoint-decouple-design.md`).
> Both `extract_ready_ips` and `resolve_endpoints` no longer exist in
> `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`.
> IPv6 bracketing is now handled uniformly by the gateway-side
> `EpSliceStore::ready_endpoints_for_port`
> (`edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs`).
> The original finding text is preserved below for historical reference.

# LinkSys webhook `extract_ready_ips` IPv6 bracketing (link-sys-F001, controller path)

**Status / verdict**: fixed (link-sys-F001, 2026-05-20); **controller path removed (2026-06-20)**

When reviewing LinkSys webhook-service endpoint findings of the form "`extract_ready_ips` builds `{ip}:{ep_port}` without IPv6 brackets / IPv6 webhook backend URL is malformed (`http://fe80::1:8080`)": **fixed (link-sys-F001, 2026-05-20)**. `extract_ready_ips` (`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`) now branches on `eps.address_type == "IPv6"` and emits `format!("[{ip}]:{ep_port}")` for IPv6 (mirroring `discovery_impl.rs`); the downstream `webhook/manager.rs` builds `{scheme}://{ep}` and feeds it to reqwest. This is the same IPv6-bracketing defect class as backends-F001, in the controller LinkSys path.

`extract_ready_ips` and `resolve_endpoints` no longer exist — the entire controller-side webhook
endpoint injection path was removed by the 2026-06-20 endpoint-decouple refactor. If a suspected
IPv6 bracketing gap is found, look at `EpSliceStore::ready_endpoints_for_port`
(`edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs`), which now handles
IPv6 bracketing uniformly for all gateway endpoint consumers.
