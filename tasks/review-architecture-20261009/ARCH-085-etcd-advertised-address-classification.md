---
id: ARCH-085
status: candidate
kind: behavior-defect
priority: P3
partition: PART-427
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

Title: Validate advertised Etcd leader IPs semantically instead of comparing spellings

EtcdLeaderElectionConfig promises to reject loopback and unspecified advertised addresses, but addr_is_routable at edgion-controller/src/config/conf_center/etcd.rs:140-150 compares a few literal strings. A valid authority such as 127.0.0.2:12101 passes because its extracted host is not exactly 127.0.0.1. Expanded IPv6 loopback and IPv4-mapped loopback spellings also miss the string checks. This is an incomplete cold configuration invariant, not a claim that configuration authors can exploit an SSRF path.

EtcdCenter::new actually invokes EtcdLeaderElection::new, which validates the config before constructing LeaderInfo. The constructor copies both grpc_addr and admin_addr without normalization into the election value. On a standby, EtcdCenter::leadership_status exposes the learned admin_addr and api::not_leader_rejection returns it in the non-public request 409 body. Advertising 127.0.0.2 points a remote caller at its own loopback rather than the leader. The currently traced concrete consumer is the Admin hint; the grpc_addr comment describes gateway rotation, but this finding does not assert that current Gateway transport implements that advertised-address consumer.

The scope is one existing validation owner, not a new network policy. Parse the supported host/port authority grammar once in the private validator; for literal hosts, reuse Common's policy-free `edgion_common::egress::classify_ip`, rejecting only `IpClass::Loopback` and `IpClass::Unspecified`. That canonical classification already normalizes IPv4-mapped IPv6. Preserve legitimate private and public addresses, ordinary IPv6, existing DNS host support, and the existing explicit `localhost` rejection. If malformed authorities must be rejected, define and test that grammar explicitly rather than introducing a URI/scheme mode as an incidental fix. Do not resolve DNS or try to prove actual reachability: this check cannot guarantee routability of a configured DNS name or deployment route. Keep advertised serialization and human identity separate from election ownership, and retain all timing, lease, serial-workqueue and TLS boundaries.

Current Controller Admin exposure classification already uses addr.ip().to_canonical().is_loopback(); the project has no general advertised-authority validator to reuse. Common's `classify_ip` returns address categories without making an allow/block decision, so it can be reused while the advertised-address owner retains its own narrow rejection policy. Do not install the outbound `EgressResolver`, apply its broader filtering rules, or reject valid private HA endpoints. Reusing the canonical literal classifier avoids another incomplete parallel list without adding a general registry or request-path allocation.

Existing Etcd config tests cover the exact 127.0.0.1 spelling and missing addresses, valid private addresses, timing and trusted bootstrap conversion. They do not cover the rest of the IPv4 loopback range, expanded IPv6, mapped loopback/unspecified literals, and valid ordinary IPv6/DNS authorities. Add focused table-driven validation cases while preserving the existing timing and direct bootstrap material tests. All source/test evidence here is static; no tests, applications or network calls ran.

Original ConfigCenter architecture, conf-center-etcd-abstraction and the existing IPv4-mapped-IP review decision were read. The ConfigCenter index refers to an absent historical election specification and contains stale storage descriptions; this claim instead rests on the current validator's explicit reject-loopback contract and actual Admin hint consumer. The prior mapped-IP decision concerns Gateway outbound classification, a separate corrected owner, and does not establish this validator's correctness. No matching pending task or earlier finding was found.
