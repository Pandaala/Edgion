---
name: backend-tls-group-key-and-endpoint-sentinel
description: Use when reviewing backend TLS peer group_key hash or endpoint port-sentinel findings; documents that DefaultHasher is acceptable here and that legacy port-80/port-mismatch sentinels were already removed.
---

# Backend TLS Peer `group_key` Uses `DefaultHasher`, and Legacy Endpoint Port Sentinels Are Already Gone

> Source: 2026-05 backend / peer-builder review (g7-006). Three sub-findings clustered around `discovery/` and `peer_builder.rs`; all three are non-issues against current code.

## (a)/(b) Port-80 sentinel and `unwrap_or(80)` vs `unwrap_or(8080)` mismatch — already removed

**Reported pattern**:
- `discovery/endpoint/discovery_impl.rs` treats port `80` as a sentinel for "no port specified, read from subset" — undocumented and inconsistent with the EndpointSlice path.
- `endpoint/endpoint_store.rs` falls back to `unwrap_or(80)` while `endpoint_slice/ep_slice_store.rs` falls back to `unwrap_or(8080)` — divergent defaults.

**Reality**: The entire `endpoint/` directory was removed in commit `43acc97 rm EndPoints`. The codebase now has only one discovery path (`endpoint_slice/`), so neither the sentinel nor the mismatched fallback exists. `unwrap_or(8080)` in `endpoint_slice/ep_slice_store.rs::get_backends_for_service` is the single port fallback path; there is no second fallback to disagree with.

**Decision rule**: If a finding cites `discovery/endpoint/...` paths or "port-80 sentinel in EndpointDiscovery", verify the file/type actually exists in HEAD. If it does not (`EndpointDiscovery` symbol absent, `endpoint/` directory absent), the finding is against stale code — close.

## (c) `DefaultHasher` for `peer.group_key` Is Not a Vulnerability

**Reported pattern**: `peer_builder.rs` (currently around line 187 in `build_peer_for_backend_tls_policy`) computes `peer.group_key = DefaultHasher::hash(sorted SAN entries)`. `DefaultHasher` is implementation-defined across Rust versions and 64-bit; a collision could let two `BackendTLSPolicy` instances with different SAN policies share the same Pingora connection pool bucket, bypassing the per-handshake SAN validation hook on a reused connection.

**Reality**:

1. **`group_key` is process-local state, never persisted or cross-process**: The "`DefaultHasher` changes between Rust versions" concern only matters for hashes that must remain stable across binary upgrades or across machines. `peer.group_key` is consumed exclusively by the in-process connection pool and is recomputed on every peer construction. Cross-version determinism is irrelevant.
2. **A different 64-bit hasher provides no security improvement**: Swapping `DefaultHasher` for `FxHash`/`AHash` keeps the same 64-bit output size; collision probability is unchanged (~2⁻⁶⁴ per distinct SAN-set pair). The suggested fix changes nothing measurable.
3. **The code is additive defense, not a risk source**: Without setting `group_key`, Pingora's default bucketing is `(addr, sni, scheme, cert_key)` — and two BackendTLSPolicies that target the same backend with the same SNI/cert but different SAN lists would already share a pool. The current code *adds* SAN-based separation on top of that. To trigger the imagined collision scenario you would need (a) two policies with identical addr/sni/cert, (b) different SAN lists, (c) a 64-bit hash collision between those two SAN lists. Conditions (a)+(b) are an unusual configuration; (c) on top of that is negligible.
4. **SAN validation result is per-handshake, not per-request**: The `upstream_tls_handshake_complete_hook` runs once on TLS handshake. Pool reuse skips the hook. So the theoretical worst case is "reuse skips re-validation" — but that worst case already exists without `group_key`, and `group_key` only narrows the reuse window further.

**Decision rule**: When a finding claims `DefaultHasher` in `peer_builder.rs::build_peer_for_backend_tls_policy` is a "deterministic hash / version stability / collision risk" vulnerability, reject it. Process-local hashing for connection pool bucketing has no cross-version requirement; another 64-bit hasher does not reduce collision rate; the code is additive defense, not a risk source. Fix suggestions to swap to `FxHash` / `AHash` / "deterministic stable hasher" are not accepted.
