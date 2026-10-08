---
name: clusterip-ipv6-no-brackets
description: Historical IPv6 address construction fix and the current native Service strategy boundary.
knowledge_type: regression-guards
---

# backends-F017 — IPv6 address construction and native Service strategy

The May 21, 2026 fix replaced `format!("{}:{}", cluster_ip, port)` with parsing
`IpAddr` and constructing `std::net::SocketAddr::new(ip, port)`. The old string
could not represent an unbracketed IPv6 literal correctly and failed closed.
That historical fix established family-independent address construction.

The native `ServiceClusterIp` match arm no longer exists. Native Route references
accept standard Service identity and a checked declared port; ordinary Services
use their proven EndpointSlice section. HTTP/gRPC detect an actual ExternalName
Service from the same captured discovery projection and construct DNS addresses
through canonical `first_safe_inet_addr`, including mapped IPv6 safety checks.
Custom outbound ClusterIP modes remain separate and are not removed by this work.

Do not apply the deleted arm's historical behavior as a current native feature
claim or as evidence against a new finding. Re-evaluate actual address builders,
Service strategy dispatch and tests when IPv6 parsing, DNS, or custom modes change.
Historical related findings: backends-F001 and link-sys-F001 concerned the same
family-independent construction issue in other consumers.
