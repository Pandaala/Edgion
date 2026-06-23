---
name: resolve-domain-ipv6-literal
description: Read when reviewing resolve_domain building format!("{}:{}", domain, port) and handing an unbracketed IPv6 literal (::1:9090) to lookup_host / fragile IPv6-literal DNS resolution (net-F001).
---

# net-F001 — resolve_domain hands unbracketed IPv6 literal to lookup_host

**Status / verdict**: fixed (net-F001, 2026-05-21)

When reviewing findings of the form "`resolve_domain` builds `format!("{}:{}", domain, port)` and hands an unbracketed IPv6 literal (`::1:9090`) to `lookup_host` / relies on resolver last-colon leniency / fragile IPv6-literal DNS resolution (net-F001)": fixed (net-F001, 2026-05-21).

`resolve_domain` (`edgion-gateway/src/util/net.rs`) now short-circuits IP literals via `IpAddr::from_str(domain)` → `SocketAddr::new(ip, port)` before any `lookup_host` call, so an IPv6 literal is parsed RFC-correctly (no unbracketed `::1:9090` ambiguity, where the string is itself a valid IPv6 address; and no resolver round-trip); the hostname and empty-string (`resolve_domain_invalid_returns_connect_error`) paths are unchanged. This also makes the `backends/mod.rs` comment "IP literals are parsed without a network round-trip" actually true. Unit test `resolve_domain_ipv6_literal` extended with a multi-group `2001:db8::1` literal. Same IPv6-authority defect class as backends-F001 / backends-F017 / link-sys-F001 / redirect-F001. Re-reports are not accepted; verify the current `resolve_domain` short-circuits literals before re-reporting.
