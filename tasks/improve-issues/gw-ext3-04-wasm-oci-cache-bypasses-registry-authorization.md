# gw-ext3-04: Wasm OCI memory cache is keyed only by digest, so a namespace without registry credentials can load another namespace's private module

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/wasm/oci/cache.rs:33` (one process-global cache), `cache.rs:74-84` (`CacheKind::key`: `"manifest:<hex>"` / `"blob:<hex>"` only), `cache.rs:267-286` (`CacheAccess::load`: no registry, repository or credential check), `edgion-gateway/src/plugins/http/wasm/oci/mod.rs:592-660` (`fetch_manifest` -> `fetch_manifest_by_digest` / `fetch_blob` -> `cached_verified_fetch`, which returns a cache hit before any registry request), `oci/mod.rs:254` (only check is "secret referenced but unresolved"), `edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm_oci.rs:36-39` (`pullSecretRef`; cross-namespace use requires a ReferenceGrant)

## Summary
Pull credentials are namespace-scoped (`pullSecretRef`, ReferenceGrant for cross-namespace), but
the verified-artifact cache that sits in front of every OCI pull is process-global and keyed only by
content digest. Once any plugin has pulled `privreg.example/team-a/filter@sha256:D` with Team A's
secret, an EdgionPlugins object in another namespace that references `...@sha256:D` (with the same
or any other registry/repository name, and no pull secret) is served the manifest and blob from
memory without ever contacting the registry. Registry authorization is therefore enforced only for
the first puller of each digest.

## Current behavior (evidence)
- `fetch_oci_bytes` -> `RegistrySession::new` checks only that a *referenced* secret was resolved
  (`oci/mod.rs:254`); a spec without `pullSecretRef` proceeds anonymously.
- For a digest reference, `fetch_manifest` calls `fetch_manifest_by_digest`, which calls
  `cached_verified_fetch(session.cache, CacheKind::Manifest, digest, ...)`. Its first statement is
  `if let Some(bytes) = cache.load(kind, hex_part) { return Ok(bytes); }` (`oci/mod.rs:653`).
- `CacheAccess::load` (`cache.rs:267-286`) checks owner count, generation and TTL only. The key
  (`cache.rs:74-84`) contains neither the registry host, the repository, nor the identity that
  authorized the original fetch.
- The blob is then fetched the same way (`fetch_blob` -> `cached_verified_fetch`), again a cache hit.
  No network request, so no 401/403 is ever observed for the unauthorized namespace.
- The cache is shared by all plugins in the gateway process (`static CACHE`), and its TTL keeps
  entries while any plugin uses them.

## Industry comparison
- Kubernetes had the identical problem with node-local image caches (a pod could run a private image
  already pulled by another tenant). It is addressed by the `AlwaysPullImages` admission plugin and,
  natively, by KEP-2535 "Ensure Secret Pulled Images" (kubelet records which credentials pulled an
  image and re-verifies access for pods using different credentials).
- Istio's Wasm image cache keys modules by image URL plus checksum and re-resolves tag references
  with the requesting `WasmPlugin`'s pull secret; it does not serve a different resource's private
  image without the configured fetch policy.

## Impact
In a multi-tenant gateway (the reason `pullSecretRef` and ReferenceGrant exist), a tenant who learns
a digest (CI logs, image lists, a public tag that was later made private, another tenant's
EdgionPlugins if readable) can execute, and through `proxy_get_configuration`-style probing
partially exfiltrate, another tenant's proprietary filter without holding that registry credential.
The bypass is silent: no registry audit entry exists for the second consumer.

## Suggested direction
Bind cache hits to authorization, Kubernetes-KEP-2535 style:
1. Key or tag entries with `(registry host, repository)` in addition to the digest, so a hit requires
   the same image coordinates; and
2. Record the credential identity (e.g. a hash of the selected registry auth, or "anonymous") that
   fetched each entry. On a hit from a different identity, perform a cheap authorization check
   against the registry (`HEAD /v2/<repo>/manifests/<digest>` with the caller's credentials) before
   returning the cached bytes; cache the positive result per identity for the entry's TTL.
3. Anonymous-public pulls can continue to share entries freely once the registry confirms anonymous
   access.

## Notes
- Checked `skills/04-review/wasm/intentional-design/bounded-oci-artifact-memory-cache.md`: it
  covers memory accounting, TTL, ownership and cancellation, and states "Credential failures are
  not artifact-cache negative entries", but does not address whether a *positive* entry may be
  served to a principal without access. No task under `tasks/` mentions it.
- Content integrity is not the issue (digests are verified before insertion); authorization is.

## Verification (independent)

Verdict: CONFIRMED. Severity P2 kept.
- `CacheKind::key` is `"manifest:<hex>"` / `"blob:<hex>"` only (`oci/cache.rs:73-83`); `CacheAccess::load` (`:267-286`) checks owners/generation/TTL only.
- `fetch_manifest_by_digest` and `fetch_blob` go through `cached_verified_fetch` (`oci/mod.rs:611-660`), which returns a cache hit before any registry request, so no 401/403 is observed for a principal without credentials.
- `RegistrySession::new` (`oci/mod.rs:249-260`) only fails a *referenced but unresolved* secret; an absent `pullSecretRef` proceeds anonymously. Same class as Kubernetes KEP-2535.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `wasm/oci/cache.rs`, `oci/mod.rs` and `plugin_configs/wasm_oci.rs` are unchanged; cache keys remain digest-only with no registry or credential binding.
- `bounded-oci-artifact-memory-cache.md` is unchanged.
