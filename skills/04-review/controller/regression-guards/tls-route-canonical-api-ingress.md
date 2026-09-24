---
name: tls-route-canonical-api-ingress
description: Use when reviewing TLSRoute raw input, API-version selection, watcher registration, or status transport.
knowledge_type: regression-guards
---

# TLSRoute uses one canonical API identity

## Conclusion and scope

TLSRoute uses `gateway.networking.k8s.io/v1` for Controller preflight, its single
typed watcher, CRUD, processing, and status writes. FileSystem, etcd, and Admin
raw input reject a missing, malformed, or alternate apiVersion before typed
deserialization can discard TypeMeta. Direct generated-type deserialization is
not an ingress validator.

The TLSRoute spec still describes SNI hostnames, parent attachments, and backend
references. This correction removes API-version conversion only; it does not
change listener resolution, stream plugins, or Gateway TLS behavior.

## Rationale and evidence

The canonical authority is `skills/00-design-rules.md` section L and
`skills/08-kubernetes/04-api-version-upgrade.md`. The user explicitly authorized
TLSRoute v1-only removal as the prerequisite to the parent-status identity fix.
That decision resolves the previous GAV-001 coordination gate for TLSRoute,
without authorizing version migrations for other kinds.

- `edgion-resources/src/resources/tls_route.rs` defines only TLSRoute v1.
  The legacy type, registry adapter, conversions, and Controller compatibility
  handler are removed together.
- `conf_center/kubernetes/version_detection.rs` uses the ordinary canonical
  preflight entry; `controller.rs` starts one TLSRoute watcher.
- `kubernetes/storage.rs` selects v1 without version discovery and checks
  raw input before parsing a DynamicObject. TLSRoute is excluded from the
  version-projection helper; only ReferenceGrant retains its separate behavior.
- `conf_center/common.rs::parse_resource_content` validates the version
  before FileSystem/etcd insertion into the raw store.
- `api/common.rs::parse_resource_and_update_version` calls the same raw
  guard before assigning an internal resource version. Create, update, and
  validation therefore cannot silently normalize a legacy document into v1.
- Kubernetes status transport derives its API from the same TLSRoute type.
  The workqueue, one-worker lifecycle, and requeue ordering are unchanged.

## Alternatives and limits

The previous v1alpha3 conversions dropped Controller-resolved attachment
fields and would also have dropped the new resolved status-controller identity.
Keeping a legacy converter would preserve an unapproved alternate API path.
The replacement is canonical ingress enforcement, not a new conversion shim.

Vendored experimental CRDs may still serve alternate versions. Their schemas
remain unchanged and Kubernetes may perform its own conversion; this does not
establish direct Edgion input support for those versions. ReferenceGrant and
the broader source-version/schema investigation are outside this correction.
The current global version policy permits separately tested source adapters;
this historical TLSRoute correction remains a v1-only boundary until an
explicit TLSRoute adapter is implemented and verified.

## Verification and re-evaluation

Regression coverage includes the FileSystem controller event stream rejecting
invalid versions before raw-store/cache publication and recovering on a later
v1 event; shared FileSystem/etcd parsing; Admin parser JSON/YAML and version
assignment modes; actual Admin create/update/validation endpoints; canonical
Kubernetes endpoint selection without discovery and raw DynamicObject parsing;
and preflight identity matching the watcher/status type.

Fresh verification on 2026-09-20: `cargo test -p edgion-controller --lib tls_route`
passed all 13 selected tests. Independent code and knowledge review returned LGTM.
Combined closure checks passed formatting, workspace all-target check/Clippy and the repository
docs, SSA, metrics, Pingora-source, transform-contract and CRD/schema guards; two unrelated
pre-existing test-only Clippy warnings remain unchanged.
No live etcd or Kubernetes conformance run is claimed by these unit tests.

Re-evaluate on new raw-input boundaries, source-version policy changes, or
changes to watch, CRUD, status, or resource-registration types.

## Resolution references

- `CAND-P3-1`: the support matrix links to the current canonical-input section, not the
  removed unresolved-discrepancy anchor; this documentation correction changes no API behavior.

- `remove-tlsroute-v1alpha3-fallback`: canonical-only implementation authorized
  as a prerequisite to `route-parent-status-canonical-identity`.
