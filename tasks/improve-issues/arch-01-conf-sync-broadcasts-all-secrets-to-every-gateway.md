# arch-01: conf_sync broadcasts every resource (including resolved secrets) to every gateway process under one shared identity

- **Category**: design-error
- **Severity**: P1
- **Scope**: large-direction
- **Location**:
  - `edgion-common/src/conf_sync/proto/config_sync.proto:22` (`ListRequest`/`WatchRequest` carry no gateway scope)
  - `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:548` (`list`), `:583` (`watch`) — no per-client filtering
  - `edgion-controller/src/services/conf_sync_ca/ca.rs:132` (`sign_client`: every gateway gets the same `CN=edgion-gateway` leaf)
  - `edgion-controller/src/services/conf_sync_ca/provision.rs:54` (one shared gateway leaf Secret per namespace)
  - `edgion-gateway/src/conf_sync/conf_client/scoped_handler.rs:13` (dedicated-Gateway projection happens only *after* receipt, on the gateway)

## Summary

The Controller serves one undifferentiated List/Watch stream per kind. Every gateway process — shared or
"dedicated" to one Gateway — receives every resource of every kind, including controller-resolved secret
material (EdgionTls private keys, BackendTLSPolicy/outbound mTLS keys, OIDC client secrets, LDAP bind
passwords, HMAC/JWT/JWE keys, key-auth credentials, LinkSys credentials). All gateways authenticate with
the same client certificate, so the controller cannot tell gateways apart and cannot scope what it sends.
Dedicated-Gateway isolation is therefore a routing-table projection, not a security boundary.

## Current behavior (evidence)

- `ListRequest { key, kind, expected_server_id }` and `WatchRequest { ..., client_id, client_name }` —
  `client_id` is self-declared and used only for bookkeeping (`client_registry.rs:40`), never for authz or
  filtering.
- `grpc_server.rs::list/watch` return the full per-kind cache (`ServerCache<T>`) to any peer that passes mTLS.
- `ca.rs::sign_client` mints a single `CN=edgion-gateway` ClientAuth leaf; `provision.rs` stores it in one
  Secret mounted by all gateway Deployments. No SPIFFE ID / SAN carries a Gateway identity.
- `scoped_handler.rs` projects only HTTPRoute/GRPCRoute/TCPRoute/TLSRoute/UDPRoute/EdgionTls/EdgionAcme for a
  dedicated process (`project_for_gateway`). EdgionPlugins, BackendTLSPolicy, LinkSys, EdgionConfigData,
  EdgionBackend etc. are not projected at all, and even the projected kinds were already received in full.
- Resolved secrets are deliberately shipped over conf_sync (`redact_opt` masks admin output only — see
  CLAUDE.md "CRD schema ↔ struct invariants").

## Industry comparison

- **Istio**: each proxy authenticates with its own SPIFFE identity; istiod computes per-proxy push contexts
  (Sidecar scoping, `discoverySelectors`), and SDS serves a secret only to a proxy whose identity is authorized
  for it (`istio-agent`/`sds` authorization by service account; gateways only get credentials for their own
  `credentialName` in their namespace).
- **Envoy Gateway**: translates per Gateway and serves each Envoy fleet only its own xDS snapshot keyed by node
  ID; one infra per Gateway is the isolation unit.
- **Kong hybrid mode**: ships full config to every DP, but Kong positions all DPs of one CP as one trust domain;
  multi-tenant isolation requires separate CPs (Konnect control-plane groups).

## Impact

- A compromised pod of any (even low-trust, internet-facing, dedicated) Gateway obtains the private keys and
  credentials of every other Gateway/tenant served by the same controller. "Dedicated Gateway" suggests an
  isolation boundary it does not provide.
- Every gateway pays memory/CPU and conf_sync bandwidth for resources it never serves; List payloads and
  relist storms grow with the whole cluster, not the gateway's share.
- The shared client leaf cannot be revoked per gateway; rotating it is a fleet-wide event.

## Suggested direction

1. **Per-gateway identity**: mint gateway leaves with an identity SAN (e.g.
   `spiffe://<trust-domain>/ns/<ns>/gateway/<name>` or a shared-pool identity), issued per Deployment
   (controller already owns the CA and the Gateway → Deployment mapping).
2. **Server-side scoping**: derive the client's scope from the verified peer certificate (never from
   `client_id`), and compute a per-scope view in the ConfigSync server: routes/TLS/plugins/policies/backends
   reachable from the scoped Gateway's listeners, and secrets only when referenced by that view. Keep shared
   processes as one scope ("shared pool") so the common case stays one cache.
3. Move the existing gateway-side `ScopedHandler` projection to the controller (keep a gateway-side assertion
   that rejects out-of-scope objects as defense in depth).
4. Option B (cheaper first step): at minimum stop shipping secret-bearing kinds/fields to dedicated processes
   that do not reference them, keyed by verified identity.

Recommendation: do (1)+(2); the dependency graph needed for (2) already exists in the controller
(`gateway_route_index`, `ref_manager`).

## Notes

- Checked `skills/01-architecture/03-controller-gateway-link/`, `skills/04-review/conf-sync/`,
  `skills/02-features/03-resources/01-gateway.md` (dedicated Gateways are described only as isolated from
  shared-listener arbitration, not as a secret boundary). No accepted-design page covers broadcast of secrets.
- The channel itself is mTLS (fail-closed when `conf_sync_security` is absent); this finding is about
  authorization and data minimization inside that channel.

## Related findings

- (ctrl-svc-06 merged below), ctrl-center-03 (unscoped core-kind ingest — the scale side), res-08 (secrets embedded as whole Secret objects — SDS-style split), gw-infra-06 (gateway dials every Etcd LinkSys). Fix together: per-gateway identity first, then server-side scoping.

## Merged from ctrl-svc-06 (shared gateway client key and rotation machinery)

ctrl-svc-06 described the same root cause from the credential side; its unique evidence is kept here and
that file was removed.

- Every gateway mounts one Secret (`edgion-conf-sync-certs`, `secret_io.rs:158-174`) that holds the shared
  client key. `skills/01-architecture/03-controller-gateway-link/00-overview.md:172,236` records that leaf
  rotation forces a fleet-wide full relist "since gateways share one client-leaf Secret".
- Defaults (`edgion-common/src/conf_sync/config.rs:343-353`): `leaf_ttl` 365 d, `ca_ttl` 3650 d,
  `lifetime_ratio` 0.5. A leaked gateway key is valid for up to a year, and the only revocation is a
  fleet-wide CA re-key.
- About 900 lines of rotation and scheduling code (`rotation.rs` 630, `cert_util.rs` 276) plus eight
  `ConfSyncRotationConfig` knobs (policy, cron schedule, timezone, emergency floor) mainly exist to
  schedule that synchronized fleet-wide reconnect.
- Industry: Istio and Linkerd issue per-proxy keys through a CSR authenticated by a projected
  ServiceAccount token. Certificates last 24 h and rotate without tearing down xDS streams.
- Extra direction: per-gateway CSR plus a short-lived leaf with a Gateway or ServiceAccount SAN, and
  non-disruptive identity swap on the gateway. With that in place, most of the scheduled/cron rotation
  policy can be deleted. A smaller first step is one leaf per gateway Deployment.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P1
- `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:549-580`: `list` validates only `expected_server_id`/`kind`, then returns `server.list_grpc_cached(&req.kind)` to any mTLS peer; `watch` likewise has no scope input.
- `edgion-common/src/conf_sync/proto/config_sync.proto:21-25`: `ListRequest { key, kind, expected_server_id }` carries no gateway scope.
- `edgion-controller/src/services/conf_sync_ca/ca.rs:129-150`: `sign_client` mints a single `CN=edgion-gateway` clientAuth leaf with no SAN identity; `provision.rs:54-110` adopts/rewrites one shared leaf Secret.
- `edgion-gateway/src/conf_sync/conf_client/scoped_handler.rs:1-14`: projection covers only route kinds, EdgionTls and EdgionAcme, after receipt on the gateway.
- No intentional-design page under `skills/04-review/conf-sync/` or `skills/04-review/security/` accepts broadcast of resolved secrets; severity kept at P1 because Gateway API's per-team Gateway model makes cross-Gateway key exposure a real multi-tenant boundary failure.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1 (unchanged)
- `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:548,583`: `list`/`watch` still serve the whole per-kind cache with no peer-derived scope; the only diff since `feature-08-30` is the tonic 0.14 `TcpIncoming` constructor (`:439`).
- `edgion-common/src/conf_sync/proto/config_sync.proto:22,44`: `ListRequest`/`WatchRequest` unchanged, no gateway scope.
- `edgion-controller/src/services/conf_sync_ca/ca.rs:132-137`: `sign_client` still mints one `CN=edgion-gateway` leaf; `conf_sync_ca/` and `scoped_handler.rs` are unchanged (`tls_identity.rs` only swapped `rustls_pemfile` for `pem_slice_iter`).
