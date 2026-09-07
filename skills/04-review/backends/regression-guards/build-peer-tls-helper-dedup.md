---
name: build-peer-tls-helper-dedup
description: Read when reviewing build_peer_from_lb_selection inlining upstream.tls construction instead of calling record_tls_to_upstream / LB paths keep a second hand-rolled copy of the TLS-metadata write (backends-F007).
knowledge_type: regression-guards
---

# backends-F007 — build_peer_from_lb_selection inlines TLS-metadata write

**Status / verdict**: fixed (backends-F007, 2026-05-24)

When reviewing findings of the form "`build_peer_from_lb_selection` inlines `upstream.tls = if use_tls { Some(BackendTlsInfo{..}) } else { None }` instead of calling `record_tls_to_upstream` / the LB paths (`Service`, `ServiceEndpointSlice`) keep a second hand-rolled copy of the TLS-metadata write / a future `BackendTlsInfo` field could be added to the helper but missed in the inline copy (backends-F007)": fixed (backends-F007, 2026-05-24).

`build_peer_from_lb_selection` (`edgion-gateway/src/backends/mod.rs`) now calls `record_tls_to_upstream(ctx, use_tls, &sni)` after the `if let Some(upstream)` block (the helper borrows `ctx` mutably, so it cannot live inside that block); the inline `BackendTlsInfo` construction is gone. All four service paths (`ServiceClusterIp`, `ServiceExternalName`, `Service`, `ServiceEndpointSlice`) now route the unconditional-write-to-avoid-stale-metadata logic through the single `record_tls_to_upstream` helper (`peer_builder.rs`), so a future `BackendTlsInfo` field is added in exactly one place. Pure no-behavior-change DRY refactor (output was already byte-for-byte identical). Re-reports of the duplicate-inline form are not accepted; verify `build_peer_from_lb_selection` calls `record_tls_to_upstream` before re-reporting.
