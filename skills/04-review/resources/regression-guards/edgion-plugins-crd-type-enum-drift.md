---
name: edgion-plugins-crd-type-enum-drift
description: Use when reviewing drift between closed EdgionPlugins CRD enums and their Rust tagged enums, including per-stage EdgionPlugin types and the shared condition KeyGet accessor.
status: fixed
finding_id: resource-02, key-get-trace-id-condition-crd-drift
closed: 2026-06-16
knowledge_type: regression-guards
---

# EdgionPlugins closed enums drifted from Rust — FIXED (reconciled + guarded)

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

The same CRD also declares a closed `KeyGet.type` enum for shared plugin conditions. When
`KeyGet::TraceId` was added, the Rust schema and Gateway runtime accepted `traceId`, but the
CRD and two count/snapshot assertions remained at eleven variants. Kubernetes therefore rejected
an otherwise valid trace-ID condition. This was fixed by adding `traceId` to the anchored
`keyGetSchema`, updating both snapshots to twelve variants, and ordering the bidirectional drift
assertion before the count assertion so a future mismatch reports its concrete set difference.

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
enum scoped to the plugins valid at that execution stage. The authoritative Rust mapping is
`HTTP_PLUGIN_STAGE_EXCEPTIONS` in `edgion_plugins/validate.rs` — a `pub const` table keyed by
`type_name()`, listing only the plugins that are NOT request-stage-only, read through
`applicable_stages_by_name()` (request / upstreamResponseFilter / upstreamResponseBodyFilter /
upstreamResponse). A reviewer who diffs the full `EdgionPlugin`
enum against just the `requestPlugins` enum will over-report — that is exactly how the original
issue mis-counted `ResponseRewrite`/`BandwidthLimit` as "missing".

**2. Why it slipped through and what now guards it**

`crd_coverage.rs` field-guards only the four fully-structural CRDs; the plugin CRD's `config` is
`x-kubernetes-preserve-unknown-fields`, so nothing compared the discriminator enums against the
Rust variants. The new guard `plugin_crd_type_coverage.rs` reads the variant universe from
`schema_for!(EdgionPlugin)` and applies three checks:
- Direction-1 (per-stage SUPERSET): for each stage, every variant valid at that stage appears in
  that stage's CRD enum. This is deliberately per-stage, not "appears in ≥1 enum" — a flattened
  check is fooled by the lenient request-stage entries (drop `ResponseHeaderModifier` from
  `upstreamResponseFilterPlugins` and the leftover `requestPlugins` entry would mask that it is
  now un-appliable at its only valid stage). Stage facts are read from
  `applicable_stages_by_name`; the guard no longer keeps its own copy of the table.
- Direction-2: every stage-enum entry names a real variant, with no duplicates (catches dead /
  duplicate entries).
- Direction-3 (`response_stage_enum_entries_are_valid_at_that_stage`): every entry in a
  *response-stage* enum is genuinely valid at that stage. Nothing else catches this — Direction-1
  is a deliberate superset check and Direction-2 only rejects names matching no variant, so a
  request-only plugin wrongly listed in `upstreamResponseFilterPlugins` would slip past both and
  an operator could apply it at a stage where it silently never runs. `requestPlugins` is exempt
  by design (see the lenient-admission note below).
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

**4. Nested `KeyGet.type` is a separate closed-enum boundary**

`KeyGet` is shared across plugins and conditions, but the condition object is outside the
preserve-unknown `config` bag and its `type` enum is enforced by Kubernetes admission. The guard
in `plugin_crd_condition_coverage.rs` compares `schema_for!(KeyGet)` with the CRD enum in both
directions. Its fixed universe-size assertion is only a schema-extraction sanity pin; it must run
after the set comparison so it cannot hide the actionable missing/stale values.

## Re-evaluation triggers

- A new `EdgionPlugin` variant is added → the guard's Direction-1 fails until the variant is
  placed in the correct stage enum(s). That is the guard working, not a regression.
- A genuine decision to make wrong-stage placement a hard admission error → then tighten the
  enums to exact-equality and upgrade the guard to a strict per-stage check (and add a `type`
  enum to `upstreamResponsePlugins`). Until then, lenient admission stands.
- A new `KeyGet` variant is added → update every operator-facing closed enum and snapshot. The
  bidirectional condition guard must report any missing or stale value before its universe-size
  sanity assertion.

## Reference

- CRD: `config/crd/edgion-crd/edgion_plugins_crd.yaml` (per-stage `type` enums)
- Rust authority: `edgion-resources/src/resources/edgion_plugins/validate.rs`
  (`HTTP_PLUGIN_STAGE_EXCEPTIONS`, `applicable_stages_by_name`), `edgion_plugin.rs`
  (`EdgionPlugin`, `type_name`)
- Guards: `edgion-resources/tests/plugin_crd_type_coverage.rs`,
  `edgion-resources/tests/plugin_crd_condition_coverage.rs`, and the explicit KeyGet snapshot in
  `edgion-resources/tests/crd_coverage.rs`; stream family:
  `edgion-resources/tests/stream_plugin_crd_type_coverage.rs`
- Integration wiring: `EdgionPlugins/FaultInjection` now has a `run_test` in both sweep blocks of
  `edgion-tests/integration/scripts/integration/run_integration.sh`

## Resolution references

- `resource-02` — reconciled the per-stage plugin discriminator enums.
- `key-get-trace-id-condition-crd-drift` — admitted the existing `traceId` accessor in the
  condition CRD and corrected the two guard snapshots; focused condition and CRD coverage tests
  passed on 2026-09-11.

## Current verification

On 2026-09-11, all eight `plugin_crd_condition_coverage` tests and all fifteen `crd_coverage`
tests passed. Workspace all-target check and Clippy passed with existing warnings, as did format,
agent-document consistency, SSA force, metrics inventory, and Pingora source-policy guards. This
does not claim a full integration or live Kubernetes run.
