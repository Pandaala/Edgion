---
name: api-version-upgrade
description: K8s API version lifecycle and controller-side upgrade strategy (alpha/beta/GA transitions)
---

# API Version Upgrade Strategy

Strategy for upgrading Gateway API and other K8s resources through the alpha → beta → GA lifecycle.

## K8s API version lifecycle

| Phase transition | Compatibility guarantee | Deprecation policy |
|------------------|-------------------------|--------------------|
| alpha → alpha | None; breaking changes allowed | May be removed directly |
| alpha → beta | Migration guide required; fields may be renamed/removed | After deprecation, retain for 3 minor versions |
| beta → GA | Fields must not be removed/renamed | After beta deprecation, retain for ≥ 3 minor versions (≥ 9 months) |
| GA → GA (v1→v2) | Extremely rare; long-term coexistence | — |

## Controller-side upgrade strategy

### 1. Version probe + fallback

Watch the highest stable version preferentially; fall back when the cluster does not support it:

```text
v1 available? → watch v1
  ↓ no
v1beta1 / v1alpha3 available? → watch the older version
  ↓ no
unsupported, return an error
```

Use the Discovery API or a lightweight `list(limit=1)` probe to detect the served versions supported by the cluster.

Default policy:

- For the same resource at any moment, choose only one version to establish a watcher
- Prefer the latest, most stable version aligned with the internal semantics
- Fall back to the older version only when the newer version is unavailable
- Do not start watchers for both `v1` and `v1alpha3` by default

Reasons:

- Watches on different API versions are different input streams
- If the cluster serves multiple versions simultaneously, watching them all may deliver the same logical object to the controller more than once
- The goal of multi-version compatibility is "supporting multiple input versions", not "consuming multiple versions simultaneously"

### 2. The meaning of API Server conversion

For the same multi-version CRD:

- The API Server persists objects in the storage version
- Whichever served version a client accesses, the API Server returns the object converted to that version
- Therefore, if the CRD correctly declares multiple versions and supports conversion, the controller usually only needs to watch the primary version

Practical judgment:

- If the old and new versions differ only by a version promotion or a few compatible fields, prefer relying on API Server conversion
- Consider multi-version watch only when API Server conversion cannot be relied upon, or when field differences between old and new are too large

### 3. Internal type unification

- Define a version-independent canonical struct internally
- Provide conversion functions: old version → canonical, new version → canonical
- Handlers, indices, cache keys, Gateway sync, and status computation operate only on the canonical type
- Old-version inputs are merely a compatibility layer; do not duplicate business logic for them

### 4. Read/write path consistency

Multi-version compatibility affects not only watch but also CRUD:

- Preflight probing must use the same version priority
- create/update/get/list/delete should hit the same primary version as watch
- The `apiVersion` in payloads sent to the API Server must align with the chosen endpoint
- Status patches must follow the same version choice

Otherwise you get inconsistencies like "watch sees v1 but writes still go to v1alpha3".

### 5. CRD multi-version coexistence

If Edgion installs CRDs itself:

- List all supported versions in `spec.versions`
- Set `storage: true` on the highest stable version
- Older versions can remain `served: true` for a migration window
- When a conversion webhook is needed, let the API Server perform unified conversion

Note:

- Within the same Gateway API release, `standard-install.yaml` and `experimental-install.yaml` may expose different served versions for the same resource
- Do not assume "for the same release and the same resource, the served versions in standard and experimental are identical"

### 6. Test matrix

Cover at least:

- The cluster has only the old version
- The cluster has both old and new versions
- The cluster has only the new version
- Watch / preflight / CRUD agree on the version choice
- Status patch can still write back when running on a fallback version

## Non-Kubernetes input sources

For non-K8s input sources such as the FileSystem, static directories, and test fixtures, the strategy differs from KubernetesCenter:

- There is no API Server, and no CRD conversion to rely on
- A runtime probe + single-active watch cannot deliver version compatibility
- Identify `apiVersion` and `kind` at the very front of "read file content"
- Convert old-version objects to the internal canonical type first, then enter the raw store, workqueue, and handler
- The queue and business logic only handle the canonical type; do not start a separate controller for the old version

Recommended flow:

```text
read file
  -> inspect apiVersion/kind
  -> deserialize legacy or current type
  -> convert to canonical internal type
  -> raw store / queue / handler
```

This way, even if a directory contains both `v1` and `v1alpha3` files, the internal processing chain stays a single model.

## Case study: ReferenceGrant v1beta1 → v1

Starting from Gateway API v1.1.0:

- `v1` is the storage version
- `v1beta1` is still served and marked `deprecated: true`
- The controller prefers watching `v1`; the API Server performs version conversion automatically
- No field changes; pure version promotion

## Case study: BackendTLSPolicy v1alpha3 → v1

In recent versions of Gateway API:

- The primary version of `BackendTLSPolicy` has become `v1`
- Some install manifests / channels may continue serving `v1alpha3`
- The controller should prefer watching `v1` and fall back to `v1alpha3` only when `v1` is unavailable
- Inputs from the older watcher should first be converted to a unified `BackendTLSPolicy` canonical type, then reuse the same handlers
- KubernetesStorage / Admin API / preflight should follow the same version-selection strategy
- In FileSystem mode, identify `v1` / `v1alpha3` at the YAML parse entry point, convert uniformly to `BackendTLSPolicy` (v1 semantics), and then enqueue

## Case study: TLSRoute v1alpha3 → v1

Starting from Gateway API v1.5.0:

- `TLSRoute` graduated to the Standard channel; primary version `v1`
- In v1.4.0 experimental install manifests, the storage version is still `v1alpha3`
- v1alpha3 → v1 is a pure version promotion; the spec field structure is identical with no differences
- The controller should prefer watching `v1` and fall back to `v1alpha3` only when `v1` is unavailable
- Reuse the multi-version pattern verified for BackendTLSPolicy: in Kubernetes mode, "prefer v1, fall back to v1alpha3, single-active watch"; in FileSystem mode, "front-load parsing and normalize to canonical v1"
- KubernetesStorage / Admin API / preflight follow the same version-selection strategy

## Edgion implementation notes

1. **Type layer**: internal structs in `edgion-resources/src/resources/` are version-independent; the conversion layer isolates API version differences
2. **Controller watch**: probe versions at init; prefer the higher version; single-active watch
3. **Path consistency**: watch, preflight, storage, and status patch use the same version selection
4. **Progressive migration**: first add new-version support → coexist multiple served versions → remove the older version after downstream clusters have migrated
