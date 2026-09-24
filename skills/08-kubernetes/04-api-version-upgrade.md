---
name: api-version-upgrade
description: Upgrade Gateway API bundles atomically while retaining one canonical model per kind and explicitly tested source-version adapters.
---

# API Version Upgrade Strategy

Edgion processes each kind through one canonical typed model. Eligible kinds
use `v1`, including TCPRoute and UDPRoute. A source GVK is a separately declared input
contract, not a second runtime model. It needs a typed source representation,
deterministic conversion, validation, status transport, and tests before
support is advertised. A CRD serving a version does not register an adapter.

Keep these axes separate: canonical model; submitted source GVK; installed
CRD served/storage versions and conversion strategy; Gateway API bundle and
Standard/Experimental channel; and individual Core/Extended/Experimental
features. `SupportedVersion` and `supportedFeatures` require independent
evidence. See the [inventory](01-gateway-api/07-version-support/01-current-support-matrix.md).

## Current version sources

Before changing code, identify each independent version source:

- committed Gateway API CRD bundle under `config/crd/gateway-api/`;
- hand-written Rust Gateway API models and Kubernetes crate dependencies;
- Go API and conformance modules under `edgion-tests/conformance/go.mod`;
- resource `apiVersion` strings in fixtures, docs, Admin API payloads, and tests;
- Controller watcher and status-patch types.

Do not describe the CRD/API bundle version and the conformance module version as
one value. For the current tree, the CRD/API bundle and Go conformance module are independently pinned to v1.6.2.

## Atomic upgrade contract

1. Select the canonical API version per kind from the committed upstream release.
2. Update the CRD bundle, Rust/Go dependencies, resource types, watchers,
   status writers, fixtures, and docs in the same change.
3. Keep one canonical processing and conf-sync model. Retain an alternate
   source adapter only when its conversion and transport contract is complete.
4. Keep watch, preflight, CRUD, and status identities consistent for each
   supported source path; a fallback probe alone is insufficient.
5. Validate FileSystem/static input against its declared source version before
   publication. Non-Kubernetes input cannot rely on API-server conversion.

## Kubernetes API Server conversion

The API server accepts only versions served by the installed CRD and converts
according to that CRD's strategy. Edgion can explicitly support a broader
source set for offline input with its own tested adapters. Kubernetes cluster
compatibility remains conditional on the installed CRD. Served versions do not
automatically become Edgion product claims.

The pinned CRD bundle and Go conformance dependency are both v1.6.2.
Standard bundles v1.0.0 through v1.6.2 form a proposed three-year fixture
window, not an automatic support promise. Experimental features need separate
evidence even when they use a v1 GVK.

## Verification

- Search the repository for the old `apiVersion`, dependency version, and type
  names; classify every remaining occurrence as historical prose or a bug.
- Verify the code-owned source inventory against typed resource GVKs and
  Controller preflight, watchers, CRUD, and status types.
- Run the relevant resource integration tests and Gateway API conformance
  profiles. HTTP/gRPC and TLS conformance remain separate runs because both
  upstream profiles reserve port 443.
- Run the repository pre-commit checks and the CRD coverage guards for schema
  changes.

## Version-promotion examples

- ReferenceGrant v1beta1 converts into the canonical v1 resource across
  Kubernetes watch and offline/Admin input. Bundle and per-source conformance
  evidence remains pending; Kubernetes input requires the CRD to serve v1beta1.
- Earlier TLSRoute and BackendTLSPolicy canonical-only corrections remain
  valid historical boundaries. They did not permanently prohibit a separately
  authorized and tested alternate source adapter.
