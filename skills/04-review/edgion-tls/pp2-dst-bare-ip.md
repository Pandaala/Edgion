---
name: pp2-dst-bare-ip
description: Read when reviewing TLS passthrough PROXY-protocol-v2 dst built from a bare IP (format!("{addr}:{port}") over info.addr) / IPv6 passthrough backend advertises 0.0.0.0:0 in the PP2 header (passthrough-F001).
---

# passthrough-F001 — TLS passthrough PP2 dst built from bare IP

**Status / verdict**: fixed (passthrough-F001, 2026-05-21)

When reviewing findings of the form "TLS passthrough PROXY-protocol-v2 dst is built from a bare IP (`format!("{addr}:{port}")` over `info.addr`) / IPv6 passthrough backend advertises `0.0.0.0:0` in the PP2 header / inconsistent with the regular TLS proxy (passthrough-F001)": fixed (passthrough-F001, 2026-05-21).

The passthrough connect loop (`src/core/gateway/routes/tls/passthrough_proxy.rs`) now carries the already-bracketed `upstream_addr_str` (`PingoraSocketAddr` Display) out of the loop alongside the stream (`Some((stream, upstream_addr_str))`) and feeds it straight into the PP2 builder — mirroring the correct sibling `proxy.rs::send_proxy_protocol_v2`. The lossy re-derivation from the bare-IP `info.addr` is gone, so IPv6 passthrough backends no longer fall back to `0.0.0.0:0`; the connect target was always correct (it used the bracketed string), so only the PP2 dst metadata changes. The structured `info.addr` (bare IP) + separate `info.port` observability fields are unchanged (only serialized into the access-log JSON, never re-parsed). Same IPv6-authority defect class as backends-F001. Re-reports of the bare-IP/missing-bracket form are not accepted; verify the PP2 builder is fed `upstream_addr_str` before re-reporting.
