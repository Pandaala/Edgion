---
name: edgion-plugins-crd-type-enum-drift
description: Use when reviewing findings that compare the EdgionPlugins CRD `type` enum against the Rust `EdgionPlugin` variants and claim "plugin type X is missing from the CRD / un-appliable" or "the CRD enum lists a stale/unknown type". Covers the per-stage enum design, what counts as real drift vs. intentional lenient admission, and the guard that now prevents it.
status: fixed
finding_id: resource-02
closed: 2026-06-16
---

# EdgionPlugins CRD `type` enum drifted from `EdgionPlugin` — FIXED (reconciled + guarded)

## Conclusion

The hand-written `config/crd/edgion-crd/edgion_plugins_crd.yaml` declares a **closed `type`
enum per plugin stage** (`requestPlugins`, `upstreamResponseFilterPlugins`,
`upstreamResponseBodyFilterPlugins`; `upstreamResponsePlugins` is intentionally free-form with
no enum). These are validated at admission (controller Admin API + the K8s API server), so a
`type` missing from EVERY stage enum is un-appliable, and a stale entry naming no real variant
is dead weight. The real drift was: `UrlRewrite` and `FaultInjection` (valid request-stage
plugins) were absent from `requestPlugins` → hard-rejected at admission; `ExtensionRef` and
`WebhookKeyGet` were dead entries (no `EdgionPlugin` variant); `ForwardAuth` was duplicated;
and `ExtensionRef` was also a dead entry in `upstreamResponseFilterPlugins`. Fixed by
reconciling the enums and adding `edgion-resources/tests/plugin_crd_type_coverage.rs`.

Two classes of re-report are **not accepted** as drift:

- "`ResponseRewrite` / `BandwidthLimit` is missing from the CRD" — false. They are **stage-scoped**:
  `ResponseRewrite` lives in `upstreamResponseFilterPlugins`, `BandwidthLimit` in
  `upstreamResponseBodyFilterPlugins`. Compare each variant against the enum **of the stage it
  applies to** (`applicable_stages` in `edgion-resources/src/resources/edgion_plugins/validate.rs`),
  not against `requestPlugins` alone.
- "`requestPlugins` lists `ResponseHeaderModifier` / `DebugAccessLogToHeader`, which are
  response-stage plugins — remove them" — **intentional lenient admission**, not a defect. The
  CRD admits them; the schema-layer validator then flags wrong-stage placement as an
  `Accepted=False`/Invalid status condition. This is the same operator-feedback-via-status
  philosophy the CRD uses for alias length vs. charset. Do not tighten these into hard
  admission rejections without an explicit design decision.

## Core Rationale

**1. The CRD enum is per-stage, mirroring `applicable_stages`, not one global list**

`EdgionPlugins.spec` has four plugin-list fields, each with its own `items.properties.type`
enum scoped to the plugins valid at that execution stage. The authoritative Rust mapping is the
exhaustive `applicable_stages(&EdgionPlugin) -> StageSet` match (request / upstreamResponseFilter
/ upstreamResponseBodyFilter / upstreamResponse). A reviewer who diffs the full `EdgionPlugin`
enum against just the `requestPlugins` enum will over-report — that is exactly how the original
issue mis-counted `ResponseRewrite`/`BandwidthLimit` as "missing".

**2. Why it slipped through and what now guards it**

`crd_coverage.rs` field-guards only the two fully-structural CRDs; the plugin CRD's `config` is
`x-kubernetes-preserve-unknown-fields`, so nothing compared the discriminator enums against the
Rust variants. The new guard `plugin_crd_type_coverage.rs` reads the variant universe from
`schema_for!(EdgionPlugin)` and applies two checks:
- Direction-1 (per-stage SUPERSET): for each stage, every variant valid at that stage appears in
  that stage's CRD enum. This is deliberately per-stage, not "appears in ≥1 enum" — a flattened
  check is fooled by the lenient request-stage entries (drop `ResponseHeaderModifier` from
  `upstreamResponseFilterPlugins` and the leftover `requestPlugins` entry would mask that it is
  now un-appliable at its only valid stage). The per-stage applicability table mirrors the six
  exception arms of `applicable_stages`; every other variant defaults to request-only.
- Direction-2: every stage-enum entry names a real variant, with no duplicates (catches dead /
  duplicate entries).
The guard is a SUPERSET check, not exact-equality: it permits the lenient extra entries
(`ResponseHeaderModifier`/`DebugAccessLogToHeader` in `requestPlugins`) without flagging them, so
it does not force a switch to hard admission rejection for wrong-stage placement. A future
async-only variant (valid only at `upstreamResponse`, which has no enum) is correctly not
required in any enum.

**3. Lenient admission is a deliberate tradeoff, related to the strict-enum invariant**

Keeping valid-but-wrong-stage plugins admissible (then flagged via status) preserves the
operator-feedback path. This is adjacent to but distinct from
[[unknown-plugin-variant-strict-enum]], which covers why `EdgionPlugin` has no catch-all variant
at the Gateway deserialize layer. Both reflect the same value: surface operator errors as status
conditions where possible, and keep the typed enum strict.

## Re-evaluation triggers

- A new `EdgionPlugin` variant is added → the guard's Direction-1 fails until the variant is
  placed in the correct stage enum(s). That is the guard working, not a regression.
- A genuine decision to make wrong-stage placement a hard admission error → then tighten the
  enums to exact-equality and upgrade the guard to a strict per-stage check (and add a `type`
  enum to `upstreamResponsePlugins`). Until then, lenient admission stands.

## Reference

- CRD: `config/crd/edgion-crd/edgion_plugins_crd.yaml` (per-stage `type` enums)
- Rust authority: `edgion-resources/src/resources/edgion_plugins/validate.rs` (`applicable_stages`),
  `edgion_plugin.rs` (`EdgionPlugin`, `type_name`)
- Guard: `edgion-resources/tests/plugin_crd_type_coverage.rs`
- Integration wiring: `EdgionPlugins/FaultInjection` now has a `run_test` in both sweep blocks of
  `edgion-tests/integration/scripts/integration/run_integration.sh`
