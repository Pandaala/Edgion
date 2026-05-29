---
name: webhook-extract-ready-ips-ipv6-brackets
description: Read when reviewing LinkSys webhook-service endpoint findings about extract_ready_ips building {ip}:{port} without IPv6 brackets / malformed IPv6 webhook backend URLs.
---

# LinkSys webhook `extract_ready_ips` IPv6 bracketing (link-sys-F001, controller path)

**Status / verdict**: fixed (link-sys-F001, 2026-05-20)

When reviewing LinkSys webhook-service endpoint findings of the form "`extract_ready_ips` builds `{ip}:{ep_port}` without IPv6 brackets / IPv6 webhook backend URL is malformed (`http://fe80::1:8080`)": **fixed (link-sys-F001, 2026-05-20)**. `extract_ready_ips` (`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`) now branches on `eps.address_type == "IPv6"` and emits `format!("[{ip}]:{ep_port}")` for IPv6 (mirroring `discovery_impl.rs`); the downstream `webhook/manager.rs` builds `{scheme}://{ep}` and feeds it to reqwest. This is the same IPv6-bracketing defect class as backends-F001, in the controller LinkSys path. Re-reports are not accepted; verify the current `extract_ready_ips` before re-reporting.
