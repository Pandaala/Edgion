---
name: cors-extension-ref-scan-skips-malformed-entry
description: Use when reviewing CORS preflight ExtensionRef scanning; malformed entries must not hide a later valid EdgionPlugins CORS reference.
status: fixed
finding_id: gw-http-proxy-req-02
closed: 2026-08-02
---

# CORS ExtensionRef Scan Skips Malformed Entries — FIXED

## Conclusion

`extract_cors_config_from_route_filters` must skip an `ExtensionRef` filter whose
`extension_ref` value is absent and continue scanning later filters. A malformed entry must not
hide a later valid `EdgionPlugins` CORS configuration.

Fix suggestions that rely only on Kubernetes CEL admission or add request-path logging are **not
accepted**.

## Core Rationale

**1. The scan is tolerant and ordered**

The function already skips non-`ExtensionRef` filters, unsupported kinds, unsupported groups, and
referenced plugin resources without CORS configuration. A missing `extension_ref` is another
unusable entry, not proof that no later entry can provide CORS configuration.

**2. `?` returns from the enclosing function**

Using `filter.extension_ref.as_ref()?` inside the loop returns `None` from the entire extraction
function. An explicit `let Some(...) else { continue; }` preserves the intended per-entry failure
boundary.

**3. Runtime hardening remains necessary outside Kubernetes admission**

The vendored Gateway API CRD rejects `type: ExtensionRef` without `extensionRef` through CEL in
Kubernetes mode. FileSystem and etcd inputs deserialize without that admission boundary, so the
gateway consumer must still tolerate the malformed shape. This is consistent with
`architecture/schema-validator-non-k8s-asymmetry.md`: concrete consumer-side completeness is the
fix, not running the full schema validator in watcher paths.

## Fix Suggestions Not Accepted

- Inject the full OpenAPI schema validator into FileSystem/etcd watcher paths — this is a broader
  architectural change already rejected by the non-Kubernetes validation precedent.
- Add a gateway `tracing` warning for the malformed entry — this function runs on the request hot
  path, where tracing is forbidden.
- Stop at the first unusable `EdgionPlugins` reference — later references may contain the active
  CORS configuration and the established scan intentionally continues.

## Re-evaluation Triggers

Re-open this decision only if:

- CORS preflight configuration stops being discovered by scanning route `ExtensionRef` filters.
- FileSystem and etcd ingestion gain an equivalent mandatory structural admission boundary.

## Reference Cases

- Finding `gw-http-proxy-req-02` — fixed 2026-08-02.
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`.
- `skills/04-review/architecture/schema-validator-non-k8s-asymmetry.md`.
