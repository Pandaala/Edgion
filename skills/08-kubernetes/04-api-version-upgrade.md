---
name: api-version-upgrade
description: Upgrade the pinned Kubernetes and Gateway API versions atomically in this pre-release workspace without adding fallback watchers or legacy conversion shims.
---

# API Version Upgrade Strategy

Edgion is pre-release and does not promise support for historical API versions.
Treat a Gateway API or CRD version upgrade as an atomic repository change, not
as a rolling compatibility program.

## Current version sources

Before changing code, identify each independent version source:

- committed Gateway API CRD bundle under `config/crd/gateway-api/`;
- Rust `k8s-gateway-api` / `k8s-openapi` dependencies;
- Go API and conformance modules under `edgion-tests/conformance/go.mod`;
- resource `apiVersion` strings in fixtures, docs, Admin API payloads, and tests;
- Controller watcher and status-patch types.

Do not describe the CRD/API bundle version and the conformance module version as
one value. For the current tree, the CRD/API bundle is v1.5.0 and the
conformance module is v1.5.1.

## Atomic upgrade contract

1. Select the new canonical API version from the committed upstream release.
2. Update the CRD bundle, Rust/Go dependencies, resource types, watchers,
   status writers, fixtures, and docs in the same change.
3. Remove old watcher types, fallback probes, conversion structs, aliases, and
   deprecated fixtures. Do not keep both versions "for migration" unless the
   user explicitly requests a multi-version support feature.
4. Keep watch, preflight, CRUD, and status patches on the same canonical
   version. A mixed-version path is a bug.
5. FileSystem/static inputs accept the canonical version only and reject other
   `apiVersion` values before they enter the raw store or workqueue.

## Kubernetes API Server conversion

The API Server converts between versions only when the installed CRD explicitly
serves those versions and has a valid conversion strategy. That upstream
mechanism is not a reason for Edgion to add fallback watchers. Edgion watches
the canonical version declared by its pinned bundle.

If multi-version cluster support is explicitly requested later, treat it as a
new feature with a defined support matrix, conversion ownership, removal plan,
and tests. Do not infer that requirement from an older fixture or from an
upstream version still being served.

## Verification

- Search the repository for the old `apiVersion`, dependency version, and type
  names; classify every remaining occurrence as historical prose or a bug.
- Verify the Controller starts one watcher per kind and writes status through
  the same version.
- Run the relevant resource integration tests and Gateway API conformance
  profiles. HTTP/gRPC and TLS conformance remain separate runs because both
  upstream profiles reserve port 443.
- Run the repository pre-commit checks and the CRD coverage guards for schema
  changes.

## Version-promotion examples

- `ReferenceGrant v1beta1 -> v1`, `BackendTLSPolicy v1alpha3 -> v1`, and
  `TLSRoute v1alpha3 -> v1` are useful search scopes, not permission to keep the
  old Rust types or runtime fallbacks.
- Historical release notes may explain the transition. Current architecture and
  feature skills describe only the canonical version.
