---
name: ipv4-mapped-ipv6
description: Read when a finding claims an IPv4-mapped IPv6 address bypasses classify_backend_addr (e.g. ::ffff:127.0.0.1 mapped loopback or ::ffff:169.254.169.254 mapped cloud metadata not blocked), or that the IPv6 branch only matches ::1 and fe80::/10.
---

# IPv4-mapped IPv6 SSRF-guard bypass in classify_backend_addr

**Status / verdict**: fixed (backends-F003, 2026-05-20)

When reviewing SSRF-guard findings of the form "IPv4-mapped IPv6 address bypasses `classify_backend_addr` / `::ffff:127.0.0.1` (mapped loopback) or `::ffff:169.254.169.254` (mapped cloud metadata) is not blocked / IPv6 branch only matches `::1` and `fe80::/10`": **fixed (backends-F003, 2026-05-20)**. `classify_backend_addr` (`edgion-common/src/utils/net.rs`) now calls `ip.to_ipv4_mapped()` in the `IpAddr::V6` branch and re-classifies any mapped address through the shared `classify_v4` helper, so mapped loopback / link-local / metadata targets are blocked while mapped public addresses stay allowed and plain `::1` is unaffected (`to_ipv4_mapped()` returns `None` for `::1`, so it still falls through to the IPv6 loopback check). The fix lives in the single classification entry point, so all three callers (`validate_backend_addr`, `validate_endpoint_backend_addr`, `pg_upstream_peer`) inherit it. The deprecated IPv4-compatible form (`::a.b.c.d`, e.g. `::127.0.0.1`) is intentionally **not** normalized — it is deprecated (RFC 4291 §2.5.5.1), rarely routable through modern stacks, and the only one-call fix (`to_ipv4()`) would wrongly downgrade `::1` to `0.0.0.1` and break the loopback check; this residual form stays covered by the NetworkPolicy defense-in-depth layer. Re-reports of the `::ffff:` mapped form are not accepted; verify the current `classify_backend_addr` V6 branch before re-reporting. A re-report specifically scoped to the `::a.b.c.d` compat form is an accepted-tradeoff close, not a code fix.
