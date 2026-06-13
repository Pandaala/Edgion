---
module: resource-config-model
title: Validation errors are untyped strings — no machine-readable error taxonomy for the control plane to act on
severity: medium
envoy_reference: xDS NACK with typed status (google.rpc.Status with error details); per-resource acceptance/rejection with structured reason codes that the management plane can consume programmatically
---

# Validation errors are untyped strings — no machine-readable error taxonomy for the control plane to act on

## Envoy's design (the proven baseline)

In xDS, the data plane responds to each resource update with an ACK or NACK. A NACK carries a
`google.rpc.Status` that includes a human-readable message AND structured `google.protobuf.Any`
error details (using `google.rpc.BadRequest` or `udpa.annotations.StatusAnnotations`). The
management plane can distinguish "schema parse failure" from "semantic validation failure" from
"dependency not found" and act accordingly — e.g., hold back the bad resource, alert ops, and
continue serving the last-known-good config.

Separately, per-resource validation in Envoy's `ConfigImpl` layer emits typed `envoy::config::core::v3::Resource`
status with typed `reason` enumerations (e.g., `RESOURCE_NOT_FOUND`, `INVALID_ARGUMENT`). This
typed taxonomy lets tooling classify errors without text scraping.

## Edgion's current design

Validation errors in the Controller flow through two collections, both `Vec<String>`:

- `validate()` returns `Vec<String>` in `ProcessorHandler` (inferred from usage at
  `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:544`).
- `preparse()` returns `Vec<String>` (`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:550`).
  **Refactor note (2026-06-13):** The `spec.preparse_errors: Vec<String>` field that was formerly embedded in `EdgionPluginsSpec` (`edgion_plugins/mod.rs:98`) has been removed from the resource type. Preparse errors on the controller side are now aggregated by `process_resource_inner` (processor.rs:550-554) and never stored in the spec. On the gateway side, `compile_plugins()` in `edgion-gateway/src/plugins/http/plugin_store.rs` stores preparse errors in `CompiledPlugins.preparse_errors` (a gateway-only field, not synced to K8s).

Both are merged into `all_errors: Vec<String>` at `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:554`, which is passed to
`update_status()`. Status conditions are set using `accepted_condition()` / `condition_true()` helpers
(`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:579`). The `message` field of each Condition is a free-form string constructed from
these error strings — no machine-readable code beyond the Gateway-API-standard `reason` field
(e.g., `"Invalid"`, `"ResolvedRefs"`).

On the Gateway side, preparse errors returned by `preparse()` are logged as `tracing::warn!` at
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:551`. There is no structured error type beyond the string, no
error code, and no way to determine from the error whether retrying will help, whether it is a
transient dependency issue, or a permanent schema violation.

The watch error constants (`edgion-resources/src/err.rs:4-9`) are typed strings but only cover sync-protocol
errors (version mismatch, server restart), not resource-content errors.

## The problem / gap

Without a typed error taxonomy:

1. **The Controller cannot distinguish error classes.** A resource that fails because its
   `secretRef` points to a non-existent Secret (a transient dependency error — the Secret may
   arrive shortly) looks identical to a resource that fails because a field value is structurally
   invalid (a permanent error that will never self-heal). Validation/preparse errors do NOT trigger
   a requeue — the resource is accepted into cache with an error Status condition and is only
   re-processed when a new K8s event arrives (e.g., the Secret appears). For the permanent error
   case the condition never self-clears, and there is no adaptive retry policy (no short-cycle for
   transient, no suppress for permanent); the behavior is driven solely by external K8s events.

2. **Operators cannot distinguish error classes.** The Status condition message is a free-form
   human string. Monitoring systems that want to alert on "resources with permanent validation
   errors" vs "resources with transient dependency errors" must parse the message text, which is
   fragile.

3. **The Gateway has no way to surface resource validation results upstream.** Because the sync
   protocol's `WatchResponse.err` field (`config_sync.proto:49`) is also a free-form string, and
   there is no per-resource acceptance/rejection signal from Gateway to Controller, the Controller
   does not know whether the Gateway successfully applied a resource or silently dropped it due to a
   parse error (e.g., the unknown-plugin-type scenario in finding 01). The `record_config_event_parse_error`
   metric (`event_dispatch.rs:306`) is a counter but carries no resource identity.

## Impact

- Error-class-based retry policy is impossible without text scraping.
- The operator experience is degraded: a resource that will never heal (bad config) looks the same
  in status as one that is waiting for a dependency (will heal shortly).
- Automated remediation systems (e.g., a controller that rolls back bad configs) cannot distinguish
  "safe to retry" from "config is fundamentally broken."
- Parse errors on the Gateway are invisible to the Controller and to the operator beyond a single
  log line and a counter with no resource key.

## Verification note

Code confirmed (second-pass). `fn validate()` and `fn preparse()` both return `Vec<String>` in the trait at `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handler.rs:67,81`. All concrete implementations follow the same signature. The `all_errors: Vec<String>` merge at `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:554` and `update_status()` at line 579 are confirmed. The `WatchResponse.err` field in `edgion-common/src/conf_sync/proto/config_sync.proto:49` is a plain `string`. The `record_config_event_parse_error` metric carries no resource key (confirmed at `edgion-gateway/src/observe/metrics/registry.rs:457` — only a `kind: &'static str` label, no namespace/name).

Correction from first-pass verification: validation/preparse errors do NOT cause a requeue. `process_resource_inner` (`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs`) continues through all pipeline stages on error, stores the resource in cache with error status, and returns `WorkItemResult::Processed`. Re-processing only happens when K8s issues a new event for the resource (e.g., a referenced Secret appears). The claim about "requeued after the same fixed delay" in the original finding is incorrect and has been corrected above. The remaining impacts (untyped status conditions, no resource key in Gateway parse-error metric) are accurate.

## Suggested direction

Introduce a structured error type for the Controller-side validation pipeline:

```rust
pub enum ResourceErrorKind {
    /// Permanent: field value violates schema. Never self-heals.
    SchemaViolation,
    /// Transient: a referenced resource does not exist yet.
    MissingDependency { kind: ResourceKind, key: String },
    /// Permanent: referenced resource exists but is incompatible.
    InvalidDependency { kind: ResourceKind, key: String, reason: String },
    /// Permanent: internal preparse failure (regex compile, TLS parse, etc.)
    PreparseFailure,
}

pub struct ResourceError {
    pub kind: ResourceErrorKind,
    pub field_path: Option<String>,
    pub message: String,
}
```

The `validate()` and `preparse()` methods in `ProcessorHandler` would return `Vec<ResourceError>`
instead of `Vec<String>`. The requeue policy would use `ResourceErrorKind` to decide delay:
`MissingDependency` gets a short retry, `SchemaViolation` gets a long (or no) retry. The Status
condition `reason` field would carry the error kind name as a stable machine-readable string.
