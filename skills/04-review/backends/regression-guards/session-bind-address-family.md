---
name: session-bind-address-family
description: Read when reviewing UDP-route get_or_create_session always binding the upstream socket to 0.0.0.0:0 / upstream UDP socket is IPv4-only / send_to returns EINVAL and silently drops every packet for an IPv6 backend (udp-F017).
knowledge_type: regression-guards
---

# udp-F017 — UDP upstream socket bind ignores address family

**Status / verdict**: fixed (udp-F017, 2026-05-26)

When reviewing UDP-route findings of the form "`get_or_create_session` always binds the upstream socket to `0.0.0.0:0` / the upstream UDP socket is IPv4-only / `send_to` returns EINVAL and silently drops every packet for an IPv6 backend (e.g. dual-stack pods) / no address-family check before the bind (F-017)": fixed (udp-F017, 2026-05-26).

`get_or_create_session` (`edgion-gateway/src/routes/udp/edgion_udp.rs`) now selects the bind address from the resolved `upstream_addr` family — `Ipv4Addr::UNSPECIFIED` for `SocketAddr::V4`, `Ipv6Addr::UNSPECIFIED` for `SocketAddr::V6` — before `UdpSocket::bind`, so the upstream socket family matches the destination and `send_to` no longer fails with EINVAL for IPv6 backends; the IPv4 path is byte-identical to before. The bind runs once per new session (then cached in `client_sessions`), rather than on established-session packets. It is still reached from the UDP packet callback and therefore remains within the data-plane logging boundary: no per-session tracing exception follows from this caching. This is the bind-family variant of the IPv6 defect class (the bracketing siblings backends-F001 / backends-F017 / link-sys-F001 / redirect-F001 / net-F001 / passthrough-F001 format an address string; this one picks the socket's address family) — both confirm "IPv6 backends are in scope; close-as-Won't-Fix is not accepted". Re-reports of the IPv4-only-bind form are not accepted; verify the current `get_or_create_session` selects `bind_addr` by `upstream_addr` family before re-reporting.
