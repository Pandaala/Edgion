---
name: mirror-tuning-annotations-vendored-crd-prune
description: Use when reviewing RequestMirror tuning fields or Edgion extension fields embedded in vendored Gateway API types; covers why HTTPRoute-path mirror tuning lives in edgion.io/mirror-* annotations.
status: fixed
finding_id: mirror-filter-vendored-crd-prune
closed: 2026-07-19
knowledge_type: intentional-design
---

# RequestMirror Tuning Moved to Annotations — Vendored CRD Prunes Extension Fields — FIXED

## Conclusion

`HTTPRequestMirrorFilter` used to carry six Edgion tuning fields
(`connectTimeoutMs`, `writeTimeoutMs`, `channelFullTimeoutMs`,
`maxBufferedChunks`, `mirrorLog`, `maxConcurrent`) plus a non-standard
`percentage` spelling. The vendored Gateway API standard CRD
(`config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml`) declares only
`backendRef` / `fraction` / `percent` under `requestMirror`, and it is a
structural schema — in K8s conf-center mode the API server **silently pruned**
every extension field (no admission error), so operators' mirror tuning fell
back to serde defaults while FS/etcd modes honored it. Fixed (2026-07-19) by
moving the tuning to route-level `edgion.io/mirror-*` annotations and slimming
the filter struct to the three standard fields.

Fix suggestions of the form "add the tuning fields back to
`HTTPRequestMirrorFilter`", "patch the vendored CRD to declare them", or
"document as FS/etcd-only" are **not accepted**.

## Core Rationale

**1. Structural-schema pruning is silent and mode-divergent.** The same YAML
worked in FS/etcd conf-center modes and no-op'ed in K8s mode with zero signal.
This is the CLAUDE.md "CRD schema ↔ struct invariants" failure mode, but on
the vendored CRD where no coverage guard exists (and none can — the CRD is
upstream-owned).

**2. Annotations are the established carrier for this exact problem.**
`edgion.io/retry-non-idempotent` predates this fix with the same rationale
(structural `HTTPRouteRetry` CRD). The mirror tuning reuses the same pipeline:
parse in `http_route_preparse.rs` (`parse_mirror_tuning_annotations`) → copy
onto every rule (`HTTPRouteRule.parsed_mirror_tuning: Option<MirrorTuning>`,
`#[serde(skip)]`) → gateway re-runs pre_parse and threads the tuning through
`compile_http_rule` → `PluginRuntime::from_httproute_filters_for_rule/_for_backend`
→ `RequestMirrorPlugin::new(filter, tuning, ns)`.

**3. The EdgionPlugins path is split, not degraded.** The `RequestMirror`
plugin variant now takes `RequestMirrorPluginConfig` = flattened
`HTTPRequestMirrorFilter` + `MirrorTuning`, preserving the historical inline
wire shape — the EdgionPlugins CRD uses `x-kubernetes-preserve-unknown-fields`,
so no pruning applies there and annotations are unnecessary.

**4. `percentage` alias removed.** The struct field is now `percent` (the
Gateway API v1.5 name, the only spelling the CRD declares); the internal
`percentage` alias was a second silent-prune trap for K8s-mode users.
Pre-release, no compat shim.

## Fix Suggestions Not Accepted

- **Patch the vendored CRD** — diverges from upstream, must be re-applied on
  every Gateway API version bump, and the CRD is shared cluster-wide with
  other controllers.
- **Document as FS/etcd-only** — keeps the silent mode divergence.
- **Re-add extension fields to `HTTPRequestMirrorFilter`** — reintroduces the
  pruning bug; the struct doc comment forbids it.

## Standing Rule for Future Reviews

Any operator-facing Edgion extension field embedded in a type that the **vendored Gateway API
CRD** serves (HTTPRoute/GRPCRoute/Gateway spec subtrees) is a silent-prune bug
in K8s mode. New route-scoped knobs go into `edgion.io/*` annotations (or an
Edgion-owned resource), never into the vendored-CRD-backed structs.

Controller-owned transport metadata is a separate class: it remains serialized
for conf_sync and carries `#[schemars(skip)]`. The mirror's `ref_denied` follows
that contract; it is not a tuning field or a CRD extension. See
[mirror authorization](../regression-guards/mirror-reference-grant-enforcement.md)
and the [schema/transport contract](../../../01-architecture/05-resources/00-schema-transport-contract.md).

## Re-evaluation Triggers

- Gateway API upstream adds official mirror tuning fields (then migrate the
  annotations to the standard fields).
- Edgion stops installing the unmodified vendored CRD.

## Reference Cases

- `tasks/todo/mirror-filter-vendored-crd-prune.md` (origin task, closed 2026-07-19)
- Sibling precedent: `edgion.io/retry-non-idempotent`
  (`skills/02-features/05-annotations/references/annotations-metadata.md`)
- Regression tests: `http_route_preparse.rs` `parses_mirror_tuning_*`,
  `mirror_tuning_reaches_resolved_rules`

## Resolution references

- `mirror-filter-vendored-crd-prune` — 2026-07-19.
