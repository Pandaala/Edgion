# new-04: Redis TLS checks Sentinel- and Cluster-discovered nodes against their announced IP, with no server-name option

- **Category**: best-practice
- **Severity**: P3
- **Scope**: local (Gateway Redis LinkSys over TLS, Sentinel and Cluster modes)
- **Location**:
  - `edgion-gateway/src/link_sys/providers/redis/transport.rs:81-91` (`ConnectionAddr::TcpTls { host, insecure: false, tls_params: None }` with the dialed host)
  - `edgion-gateway/src/link_sys/providers/redis/backend.rs:323-329` (Sentinel primary from `get-master-addr-by-name`), `:841-857` (learned Sentinels from the `ip` field), `:861-884` (Cluster nodes from `CLUSTER SLOTS`), `:370` (MOVED target)
  - `edgion-common/src/link_sys_core/outbound_tls/redis.rs:83-91` → `link_sys_core/tls.rs:26-29` (`reject_unsupported_identity_policy`: outbound `hostname`/SAN policy refused for Redis), `resolve_validated_outbound_tls_audited(.., provider_can_disable = false, ..)` (`verify: false` refused)

## Summary

In Sentinel and Cluster modes, Edgion dials addresses that the servers report. By default these are
IP literals: Sentinel's `get-master-addr-by-name` and `SENTINEL sentinels` `ip`, and Cluster's
`cluster-preferred-endpoint-type ip`. The TLS identity check uses that exact host, so rustls requires
an **IP SAN** for each node. The canonical outbound TLS policy offers a `hostname` override, but
Redis rejects it as "unsupported by this transport", and Redis also refuses `verify: false`.
Deployments whose certificates carry only DNS names (the usual case for cert-manager issued
certificates) therefore cannot use TLS with Sentinel or Cluster. The only failure shown is
"Redis TLS verification failed".

## Current behavior (evidence)

- `Transport::info` builds `TcpTls` from the dialed host (`transport.rs:82-88`). redis-rs derives
  the rustls `ServerName` from that host, so an IP host verifies against IP SANs only.
- The Sentinel primary, learned Sentinels, Cluster primaries and MOVED targets all come from server
  replies (`backend.rs:328`, `:850-852`, `:868-883`, `:370`) and are IPs unless the servers enable
  hostname announcement.
- `PreparedRedisTls::redis_tls_material` runs `build_effective_rustls_client_config`, which calls
  `reject_unsupported_identity_policy` and refuses any `hostname`/`subjectAltNames` setting. Resolution
  uses `provider_can_disable = false`. The operator therefore cannot pin a server name, and cannot
  relax hostname checking while keeping chain verification.

## Industry comparison

- go-redis `TLSConfig.ServerName` (one server name for every node), Lettuce
  `SslVerifyMode.CA` (verify the chain without the hostname), and redis-py `ssl_check_hostname=False`
  with `ssl_cert_reqs=required` all support this deployment shape without disabling verification.
- Envoy `redis_proxy` takes an upstream `sni`/SAN matcher in its `UpstreamTlsContext` that does not
  depend on the discovered address.

## Impact

A TLS-protected Sentinel or Cluster deployment fails to initialize unless every node certificate
contains every IP that a node can announce (including after rescheduling) or the servers are
reconfigured to announce hostnames. This is an interoperability gap, not a bypass. The behavior is
fail-closed.

## Suggested direction

Support the canonical outbound `hostname` (server-name) policy for Redis: verify every discovered
node's certificate chain against the configured name instead of the dialed IP. redis-rs accepts a
custom rustls `ClientConfig`/verifier through `TlsConnParams`. Keep `verify: false` refused.
Document the IP-SAN requirement until this is implemented.

## Verification (independent)

- **Verdict**: CONFIRMED (P3), verified by the parent on feature-10-28.
- `edgion-common/src/link_sys_core/tls.rs:314-322`: `reject_unsupported_identity_policy` refuses any hostname/subjectAltNames override for LinkSys transports, so discovered-IP dials cannot be verified against a DNS-name certificate.
- Impact limited to Sentinel/Cluster + TLS deployments whose node certificates lack IP SANs; severity kept at P3.
