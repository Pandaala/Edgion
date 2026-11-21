---
name: conf-center-etcd-abstraction
description: Review the explicit trusted ConfCenter bootstrap boundary and its shared Etcd TLS/options implementation.
knowledge_type: intentional-design
---

# ConfCenter Etcd bootstrap boundary

## Current contract

Controller `config/conf_center/etcd.rs::EtcdConfig::to_bootstrap_config` reads
UTF-8 local PEM files and returns Common's `EtcdBootstrapConfig`. Authentication
is direct `BasicAuthMaterial`; TLS is direct `TlsMaterial`, with a zeroizing
private-key buffer. This trusted bootstrap input has no Kubernetes source,
reference, Grant or synthetic Secret. It must never be treated as resource
reference authority.

`EtcdLinkClient::from_bootstrap` captures global TLS defaults only when a local
TLS section is present. The private borrowed bootstrap adapter and actual
LinkSys adapter enter the same effective TLS precedence/materialization core.
A disk CA shadows the complete inherited validation block. Verify is inherited.
A declared global client reference takes priority over a disk identity, and its
missing material or failed proof cannot fall back to disk credentials. A partial
disk cert/key pair stays inert, but every declared file is read and read errors
still fail conversion. No TLS section leaves SDK-native endpoint TLS handling
unchanged; this is not a guarantee about SDK trust roots.

Controller injects its existing dial and always-on HTTP/2 keepalive defaults at
conversion. The shared options mapper injects no Controller defaults into
operator-configured LinkSys clients. Ordinary connections keep the configured
unary request timeout; dedicated election connections omit it. A request-only
ordinary connection does not acquire the fallback dial timeout, while an
otherwise empty election connection retains that fallback.

Common stores one immutable connection input for initialization, endpoint
probes, monitor recovery and dedicated elections. Resource construction retains
the actual LinkSys generation; bootstrap construction retains prepared SDK
options. These paths do not re-read global TLS during reconnect. The upstream
SDK may clone and parse its own PEM/options; zeroization is not an end-to-end
SDK guarantee.

## Historical decision and re-evaluation

The 2026-04-26 P1-09 review accepted a synthetic Secret/reference conversion to
reuse the former detached `build_connect_options` entry. That conclusion is
superseded for the reference-unification task: actual-owner proof makes
fabricated resource authority unsuitable, and the original re-evaluation trigger
was reached by changing TLS/reference structure. Reuse remains required and is
now achieved through private borrowed inputs rather than synthetic resources.
Historical test results are not current verification.

Construction/file errors remain `anyhow::Result`; runtime writer operations
retain typed `ConfWriterError`. This is a layered error contract. A new client
trait is not required solely for mock convenience: `EtcdStorage` and `CenterApi`
remain the storage abstraction, and production has one Etcd client implementation.

## Advertised leader addresses

ARCH-085 reuses Common's policy-free `egress::classify_ip` for the literal host
of each advertised gRPC/Admin address. The local policy rejects only loopback
and unspecified literals, including IPv4-mapped IPv6; private, link-local,
unique-local and public literals retain their existing acceptance. Exact
`localhost` and empty-address rejection remain in place. This is not the
outbound egress filter: DNS names are not resolved, advertised strings are not
rewritten, and neither comprehensive authority syntax nor actual reachability
is established. Controller config tests exercise both advertised fields.
Re-evaluate these decisions against concrete fault-injection or backend needs,
rather than suppressing unrelated current defects.

## Source and review scope

- `edgion-controller/src/config/conf_center/etcd.rs`
- `edgion-controller/src/conf_mgr/conf_center/etcd/center.rs`
- `edgion-common/src/link_sys_core/providers/etcd/client.rs`
- `edgion-common/src/link_sys_core/providers/etcd/config_mapping.rs`
- `edgion-common/src/link_sys_core/outbound_tls.rs`

This contract does not change workqueue processing, etcd lease/CAS behavior,
endpoint discovery, message bounds, egress classification or Gateway API fields.
The reference-unification task closed on 2026-11-12; its runtime and
conformance evidence, with revisions and limits, is summarized in the
[resource reference contract](../../../01-architecture/00-common/06-resource-reference-contract.md#verification-evidence).
