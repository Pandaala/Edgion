---
name: clusterip-ipv6-no-brackets
description: ClusterIP backend address building now parses cluster_ip as IpAddr and builds the SocketAddr directly, so IPv6 ClusterIP services work. Reject re-reports claiming IPv6 ClusterIP backends always fail BackendAddressParsingFailed.
---

# backends-F017 — ClusterIP backend address handles IPv6 (no manual brackets)

**Status:** ✅ fixed · 2026-05-21
**Severity:** Medium (BadLogic / FeatureGap) — ClusterIP-type backends were non-functional on IPv6 / dual-stack clusters (fail-closed, no mis-route)

## Conclusion

The `EdgionService::ServiceClusterIp` arm in `edgion-gateway/src/backends/mod.rs` now parses
`cluster_ip` as `std::net::IpAddr` and builds the address via
`SocketAddr::Inet(std::net::SocketAddr::new(ip, port))`, so IPv6 ClusterIP literals
(`fd00::1`) produce a valid `SocketAddr` without manual bracketing.

Re-reports of the form "ClusterIP build does `format!(\"{}:{}\", cluster_ip, port)` and drops
IPv6 brackets", "IPv6 ClusterIP service always returns `BackendAddressParsingFailed`", or
"ClusterIP backends don't work on IPv6 clusters" are **not accepted** — none are true anymore.
Verify the current `ServiceClusterIp` arm parses to `IpAddr` before re-reporting.

## What was wrong (before the fix)

The arm built `format!("{}:{}", cluster_ip, port)` then `.parse::<SocketAddr>()`. For an IPv6
`cluster_ip`, the unbracketed string (`fd00::1:8080`) is not a valid `SocketAddr`, so the parse
failed with `EdgionStatus::BackendAddressParsingFailed`. The failure was **fail-closed** (clean
error, no connection to a wrong host) — so this was a feature gap ("does Edgion support IPv6
ClusterIP?"), not a silent mis-route like backends-F001's probe defect.

## Why the fix is correct

`cluster_ip` is always a bare IP literal (Service `spec.clusterIP`), so parsing it as `IpAddr`
and constructing `std::net::SocketAddr::new(ip, port)` is the canonical, family-agnostic
build — it handles both IPv4 and IPv6 with no string manipulation. The result is wrapped in
pingora's `SocketAddr::Inet(..)`, identical to the `ServiceExternalName` arm at the same site,
so the downstream `validate_backend_addr` / `create_tls_peer` calls are unaffected. Headless
(`"None"`), empty, and garbage `cluster_ip` strings still fail with the same
`BackendAddressParsingFailed`, so non-IPv6 behavior is unchanged.

This is the same IPv6 defect class as backends-F001 (probe URLs) and link-sys-F001 (controller
path, since removed by the 2026-06-20 endpoint-decouple refactor), both fixed — confirming
IPv6 backends are in scope for Edgion.

## Fix Suggestions Not Accepted

- "Branch on `is_ipv6` and `format!(\"[{}]:{}\")` like discovery_impl.rs:38" — works, but the
  ClusterIP path has no `address_type` marker; parsing to `IpAddr` is simpler and needs no
  family branch.
- "Close as Won't-Fix — IPv6 ClusterIP is out of scope" — rejected; backends-F001 and
  link-sys-F001 establish IPv6 backend support as in scope.

## Re-evaluation Triggers

Re-open only if:

- `cluster_ip` stops being a bare IP literal (e.g., switches to a hostname-based target that
  needs DNS resolution).

## Reference Cases

- backends-F017 (review finding, found during backends-F001 deep review 2026-05-20, fixed
  2026-05-21)
- backends-F001 / link-sys-F001 — same IPv6-address-build defect class.
- `edgion-gateway/src/backends/discovery/endpoint_slice/discovery_impl.rs:38` — the
  `format!("[{}]:{}", address, port)` IPv6 bracketing pattern for the `address_type`-tagged
  EndpointSlice path.
