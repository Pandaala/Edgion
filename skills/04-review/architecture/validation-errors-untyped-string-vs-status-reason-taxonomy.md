---
name: validation-errors-untyped-string-vs-status-reason-taxonomy
description: Use when reviewing findings that compare Edgion against Envoy/xDS and claim "validation/preparse errors are untyped Vec<String> / no machine-readable error taxonomy / control plane cannot programmatically classify errors / no typed ResourceErrorKind enum / cannot distinguish transient MissingDependency from permanent SchemaViolation / operators must text-scrape the status message / no error-class-driven requeue policy". Covers why the machine-readable taxonomy already exists as the Gateway API `.status.conditions[].reason` field (backed by a typed ResolvedRefsError enum + ~30 typed reason constants), and why threading a parallel ResourceErrorKind through validate()/preparse() is a large cross-cutting refactor with no net operator capability.
status: accepted-tradeoff
finding_id: resource-04
closed: 2026-06-14
---

# Validation errors are untyped `Vec<String>` — ACCEPTED-TRADEOFF (the machine-readable taxonomy already exists as the Gateway API `.status.conditions[].reason` field)

## Conclusion

`validate()` and `preparse()` in `ProcessorHandler`
(`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handler.rs:67,81`)
return `Vec<String>`, and `process_resource_inner` merges them into
`all_errors: Vec<String>` (`processor.rs:544-554`). That string vector is only the
**internal human-readable message** carrier. The **machine-readable error
classification** the finding asks for already exists, at the right layer: the
Gateway-API-standard `.status.conditions[].reason` field, backed by a typed
`ResolvedRefsError` enum and ~30 typed `condition_reasons::*` constants in
`status_utils.rs`. This is an **accepted design tradeoff**, not a defect.

Fix suggestions of the form "introduce a `ResourceErrorKind` enum
(`SchemaViolation` / `MissingDependency` / `InvalidDependency` / `PreparseFailure`)
and change `validate()`/`preparse()` to return `Vec<ResourceError>` / drive an
error-class-based requeue policy off the kind / carry the kind name into the
status `reason`" are **not accepted** as a narrow review fix.

## Core Rationale

**1. The machine-readable taxonomy already exists — as the Gateway API condition `reason`**

`status_utils.rs` defines a `condition_reasons` module with ~30 typed reason
constants: `SecretNotFound`, `BackendNotFound`, `PluginMetaDataNotFound`,
`RefNotPermitted`, `InvalidKind`, `Invalid`, `InvalidCACertificateRef`,
`DecodeError`, `MissingKeys`, `UnsupportedValue`, etc. The `.status.conditions[].reason`
field is *the* canonical machine-readable error code in the Kubernetes ecosystem —
it is exactly what `kubectl`, controllers, and monitoring systems consume, without
parsing the free-form `message`. The finding's premise ("no machine-readable code
beyond the Gateway-API-standard `reason` field") undersells that field: that field
*is* the taxonomy, and it sits at the source-of-truth layer rather than being
reconstructed from a data-plane round trip.

**2. The proposed `ResourceErrorKind` variants map one-to-one onto what already ships**

- `MissingDependency { kind, key }` → already a typed `ResolvedRefsError` enum
  (`status_utils.rs:197`) surfaced as `ResolvedRefs=False` with a deterministic
  typed reason (`SecretNotFound` / `PluginMetaDataNotFound`) chosen by
  `pick_resolved_refs_reason` (`edgion_plugins.rs:178-200`).
- `SchemaViolation` / `PreparseFailure` → already `Accepted=False` with reason
  `Invalid` (`edgion_plugins.rs:170-176`); `edgion_stream_plugins.rs:172-183` even
  adds a dedicated `LegacySchemaDeprecated` reason mapping, locked by the tests
  `update_status_maps_structural_error_to_invalid_reason` /
  `update_status_maps_legacy_marker_to_dedicated_reason` /
  `update_status_uses_invalid_reason_for_non_legacy_errors`.
- `InvalidDependency { kind, key, reason }` → already covered by the `Invalid*Ref`
  reason family (`InvalidCertificateRef`, `InvalidCACertificateRef`,
  `InvalidPluginMetaDataRef`, `InvalidStreamPluginsRef`).

Internalizing a parallel `ResourceErrorKind` into the `validate()`/`preparse()`
return type would **duplicate** this existing reason taxonomy across the core
`ProcessorHandler` trait and every concrete resource handler, while adding no
operator-visible capability the `reason` field does not already provide.

**3. The requeue-policy and upstream-feedback motivations are already settled tradeoffs**

- "Error-class-driven adaptive requeue (short retry for transient, none for
  permanent)" is exactly what the **controller-01** accepted tradeoff
  (`handler-requeue/status-write-no-retry-accepted.md`) deliberately rejected at
  this layer: `requeue_with_backoff` was removed because the layer cannot safely
  distinguish transient from permanent, so a blind class-based retry would loop
  forever on permanent errors. Healing is event-driven (the missing Secret arrives
  → `MODIFIED` → reprocess).
- "The Gateway should surface per-resource validation results back to the
  Controller" is the **sync-01** accepted tradeoff
  (`architecture/no-ack-nack-control-plane-validation.md`): Edgion validates at
  control-plane ingest and surfaces the result as a `.status` condition instead of
  a data-plane ACK/NACK round trip.

**4. It is a large cross-cutting refactor, not a narrow review fix**

Changing the `validate()`/`preparse()` signature touches the core
`ProcessorHandler` abstraction, every concrete handler implementation (every
resource type), and — to be useful — the requeue path and possibly the
`config_sync.proto` `WatchResponse.err` field. That is medium-risk cross-module
work and belongs in `tasks/todo/` via brainstorming if a concrete consumer ever
materializes — not this workflow.

## Fix Suggestions Not Accepted

- "Introduce `ResourceErrorKind` / `ResourceError` and return `Vec<ResourceError>`
  from `validate()`/`preparse()`" — duplicates the existing Gateway-API `reason`
  taxonomy across the trait and every handler with no net operator capability.
- "Drive an error-class-based requeue policy (short retry for `MissingDependency`,
  none for `SchemaViolation`)" — reintroduces the transient/permanent distinction
  that controller-01 deliberately removed from this layer; healing is event-driven.
- "Carry the error-kind name into the status `reason` so tooling can classify
  without text-scraping" — the `reason` field already carries a typed code
  (`SecretNotFound` vs `Invalid` etc.); tooling already classifies on it.
- "Add per-resource accept/reject feedback from Gateway to Controller" — this is
  the sync-01 no-ack-nack tradeoff; validation runs at control-plane ingest.

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A concrete in-tree consumer needs to branch on error class *programmatically in
  Rust* (not via the K8s `.status` condition `reason`) — e.g. an internal
  remediation loop that the Gateway-API condition cannot serve.
- A non-K8s config source is added that has no `.status` condition to carry the
  `reason`, removing the layer the machine-readable taxonomy currently lives on.
- The requeue layer gains a safe transient/permanent classifier (which would also
  re-open controller-01), making an error-kind-driven retry policy actually
  implementable without infinite loops on permanent errors.

## Reference Cases

- `tasks/review-envoy/resource-04-validation-errors-not-per-resource-typed.md`
  (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handler.rs:67,81`
  (`validate()` / `preparse()` → `Vec<String>`, internal message carrier)
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_utils.rs:18-127`
  (`condition_types` / `condition_reasons` — the typed reason taxonomy) and `:197`
  (`ResolvedRefsError` enum)
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs:158-200`
  (errors → typed `Accepted` / `ResolvedRefs` conditions via `pick_resolved_refs_reason`)
- `architecture/no-ack-nack-control-plane-validation.md` (sibling sync-01: control-plane
  ingest validation replaces data-plane ACK/NACK)
- `handler-requeue/status-write-no-retry-accepted.md` (controller-01: this layer
  deliberately does not do transient/permanent requeue distinction)
