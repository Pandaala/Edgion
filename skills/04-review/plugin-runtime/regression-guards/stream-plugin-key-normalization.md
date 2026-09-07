---
name: stream-plugin-key-normalization
description: Historical review evidence for Redundancy / L4 route handlers; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Stream plugin key normalization

## Recorded conclusion

`GlobalTcpRouteManagers::resolve_stream_plugin_store_key` (`gateway/routes/tcp/conf_handler_impl.rs`), `GlobalTlsRouteManagers::resolve_stream_plugin_store_key` (`gateway/routes/tls/conf_handler_impl.rs`), and `GlobalUdpRouteManagers::resolve_stream_plugin_store_key` (`gateway/routes/udp/conf_handler_impl.rs`) were three byte-identical private methods reading the `EDGION_STREAM_PLUGINS` annotation and qualifying bare values with the route namespace (fallback `default`). Consolidated into `pub(crate) fn resolve_stream_plugin_store_key(metadata: &ObjectMeta) -> Option<String>` in `gateway/routes/common.rs` (sibling of the existing `is_core_service_backend_ref` / `effective_hostnames` / `filter_accepted_parent_refs` shared helpers), taking `&ObjectMeta` directly so any future L4 route type (e.g. a new stream route) reuses the same contract without copying the body. Pure refactor — semantics identical. The issue file listed only TCP+TLS; UDP was folded into the same fix because leaving the third copy behind defeats the dedup. Unit tests cover (a) absent annotation, (b) whitespace-only, (c) already-qualified `ns/key`, (d) bare key with route namespace, (e) bare key with no namespace (`default` fallback). Re-reports of the per-route-type copy form are not accepted; new L4 route handlers must call `routes::common::resolve_stream_plugin_store_key(&route.metadata)`. Orthogonal to `plugins-stream-FB01` which tracks the `listener_builder.rs` site that does **not** perform this normalization at all.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `plugins-stream-F002` — 2026-05-26.
