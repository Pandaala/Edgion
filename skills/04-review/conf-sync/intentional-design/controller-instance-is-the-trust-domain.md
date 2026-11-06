---
name: controller-instance-is-the-trust-domain
description: Use when reviewing findings that claim conf_sync broadcasts every resource (including resolved secrets) to every gateway, that all gateways share one client leaf / CN=edgion-gateway identity, that List/Watch has no per-gateway scope, or that a dedicated Gateway (EDGION_GATEWAY_SCOPE) does not isolate secrets.
status: accepted-tradeoff
finding_id: arch-01, ctrl-svc-06
closed: 2026-11-06
knowledge_type: intentional-design
---

# One Controller instance is one conf_sync trust domain — ACCEPTED-TRADEOFF

## Conclusion and scope

Every gateway process served by one Controller instance belongs to one trust domain.
Within that domain the Controller serves one undifferentiated List/Watch stream per
kind, including controller-resolved secret material (TLS private keys, outbound mTLS
keys, OIDC/LDAP/HMAC/JWT/key-auth credentials, LinkSys credentials), and every gateway
authenticates with the same `CN=edgion-gateway` client leaf. This is the accepted
design, not a defect.

A dedicated Gateway process (`EDGION_GATEWAY_SCOPE` / `EDGION_GATEWAY_UID`) is a
traffic, listener-arbitration and process-resource isolation unit. It is **not** a
secret or tenant isolation boundary: it receives the full per-kind stream and only
projects Routes, EdgionTls and EdgionAcme after receipt.

Edgion provides no tenant isolation inside one Controller instance. The only
supported conf_sync isolation unit is the Controller instance: the conf_sync CA and
gateway client leaf Secrets are created in the Controller's own namespace
(`services/conf_sync_ca/provision.rs`, `secret_io.rs`), so a Controller installed in
another namespace has a separate root and its gateways cannot authenticate to the
first one. The shipped manifests assume a single install; a per-tenant deployment
is operator-built and carries the caveats under "Alternatives and limits".

## Rationale and evidence

Current behavior (verified 2026-11-06 on `feature-10-28`):

- `edgion-controller/src/conf_sync/conf_server/grpc_server.rs` `list`/`watch` validate
  only `expected_server_id` and `kind`, then serve the shared per-kind cache.
- `edgion-common/src/conf_sync/proto/config_sync.proto` `ListRequest`/`WatchRequest`
  carry no scope; `client_id`/`client_name` are self-declared bookkeeping labels and
  must never be used for authorization.
- `edgion-controller/src/services/conf_sync_ca/ca.rs` `sign_client` mints one
  `CN=edgion-gateway` clientAuth leaf; `provision.rs` stores it in one Secret
  (`edgion-conf-sync-certs`) mounted by every gateway Deployment of that namespace.
- `edgion-gateway/src/conf_sync/conf_client/scoped_handler.rs` projects only route
  kinds, EdgionTls and EdgionAcme for a dedicated process.

Why accepted:

- A single shared gateway fleet is one trust domain by construction; per-gateway
  scoping would add no protection there.
- Separating trust domains at the Controller-instance level (its own namespace and
  CA) is far cheaper than per-gateway identity and per-identity views inside one
  Controller, and keeps the common shared-fleet case on one cache.
- Comparable to Kong hybrid mode as described in the arch-01 finding (data planes of
  one control plane form one trust domain); this comparison was not re-verified
  against Kong documentation.

## Alternatives and limits

Rejected alternative (from arch-01 and ctrl-svc-06): per-gateway client identity
(SPIFFE-style SAN per Deployment or CSR with projected ServiceAccount token),
server-side per-identity scoping computed from the route/reference graph,
moving `ScopedHandler` projection to the Controller, and short-lived per-gateway
leaves. This is a large cross-layer change (CA identity model, conf_sync server
views, dependency-graph scoping, gateway assertions, rotation) whose benefit exists
only for single-Controller multi-tenant deployments, which are not a supported
isolation model.

Accepted cost and risk:

- Compromise of any gateway pod exposes every secret and credential served by its
  Controller instance, including those used only by other (dedicated) Gateways.
- The shared client leaf (default `leaf_ttl` 365 days) cannot be revoked per gateway;
  revocation is a fleet-wide CA re-key (`rekey_conf_sync_ca.sh`), and leaf rotation
  forces a fleet-wide relist.
- Each gateway holds and syncs resources it never serves; List/relist cost scales
  with the Controller's whole view.

Operators who need cross-team secret isolation must not rely on dedicated Gateways
under one Controller.

Per-tenant Controller deployment caveats (verified 2026-11-06, not productized):

- Shipped RBAC (`cicd/deploy/kubernetes/controller/rbac.yaml`) is a cluster-wide
  `ClusterRole`/`ClusterRoleBinding` named `edgion-controller` that grants Secret
  access in every namespace, and the manifests target `edgion-system`. A tenant
  install needs operator-written, uniquely named RBAC restricted to that tenant's
  namespaces; `watch_namespaces` limits watches, not permissions.
- `EdgionGatewayConfig` is cluster-scoped and watched with `spawn_cluster`
  regardless of `watch_namespaces`
  (`conf_mgr/conf_center/kubernetes/controller.rs`), so every tenant Controller
  ingests and broadcasts every EGC. GatewayClass is cluster-scoped too and is
  separated only by `controller_name`.
- The leader Lease defaults to `edgion-controller-leader` in `POD_NAMESPACE`
  (`config/conf_center/kubernetes.rs`); an explicit shared `lease_namespace` would
  make tenant Controllers contend for one Lease.
- Gateways mount `edgion-conf-sync-certs` from the Controller's namespace, so a
  tenant's gateways run in (or receive that Secret in) its Controller's namespace.

## Verification and re-evaluation

Verification is source inspection only (cited files above); no runtime test was run
for this decision.

Re-evaluate when any of these becomes true:

- The product commits to multi-tenant isolation under one Controller instance, or
  documents a dedicated Gateway as a security/tenant boundary.
- conf_sync gains a peer-verified gateway identity (then `client_id` must still not
  become an authorization input; derive scope from the verified certificate).
- Per-tenant Controller deployment stops being viable (for example the CA or leaf
  Secret moves out of the Controller's namespace or becomes cluster-shared), or
  multi-tenant installation becomes a supported deployment (then the RBAC, EGC and
  Lease caveats above must be resolved).
- Related scale findings (ctrl-center-03 unscoped core-kind ingest, gw-infra-06
  gateway-side Etcd LinkSys dialing, res-08 whole-Secret embedding) are redesigned
  in a way that introduces per-gateway views anyway.

## Resolution references

- arch-01 (P1, CONFIRMED/STILL-VALID, reclassified as accepted tradeoff 2026-11-06):
  conf_sync broadcasts every resource including resolved secrets to every gateway
  under one shared identity.
- ctrl-svc-06 (merged into arch-01): shared gateway client key and fleet-wide rotation
  machinery.
