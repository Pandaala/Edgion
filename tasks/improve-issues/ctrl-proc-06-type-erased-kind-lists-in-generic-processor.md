# ctrl-proc-06: The generic ResourceProcessor hard-codes per-kind behavior through `dyn Any` downcast lists and kind-string checks

- **Category**: over-design
- **Severity**: P3
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_pipeline.rs:13-185` (`has_non_native_status`, `replace_status`, `clear_current_status`, `capture_current_status`, `filter_current_route_status`, `filter_current_native_attachment_status`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs:121-185` (unknown-fields sidecar, `seed_existing_status` BTP/EBTP special case)
  - `processor.rs:803-818,909-933` (`matches!(self.kind, "BackendTLSPolicy" | "EdgionBackendTrafficPolicy")`, `notify_policy_ancestor_change` for every kind)
  - `processor.rs:1085-1101` (`requeue_all_keys` special-cases `"GatewayClass"`)
  - `policy_ancestor_capacity.rs:107-110`

## Summary
`ResourceProcessor<K>` is generic, but much of its pipeline dispatches on the concrete type at runtime.
It uses `obj as &dyn Any` followed by macro-generated `downcast_mut::<T>()` chains over hand-maintained type
lists, appearing in six or more separate lists. It also uses `self.kind` string comparisons. When a
type is missing from a list, the operation silently does nothing. For example, `replace_status` returns
`Ok(())` for an unlisted type, so the caller believes the status was replaced. Adding a status-bearing kind
means updating every list consistently, and the compiler does not help.

## Current behavior (evidence)
- `status_pipeline.rs:38-74`: `replace_status` tries 17 types and ends with `Ok(())`. Its callers
  (`processor.rs:568,604,846`) treat success as "status replaced". A new kind left out of the list silently
  keeps stale or foreign status, and the partitioned-status projection is skipped.
- `clear_current_status`, `capture_current_status`, `filter_current_route_status` and the "partitioned"
  macro each maintain separate, slightly different type lists (`status_pipeline.rs:76-185`).
- `processor.rs:121-150` keeps another four-type list for `unknown_fields` round-tripping through
  `serde_json::Value`, because `seed_existing_status` serializes the whole object to JSON and back just to
  insert `status` (`processor.rs:153-185`). The BTP/EBTP special case exists only because that round-trip
  breaks for those types.
- Policy-specific logic lives inline in the generic path (`processor.rs:803-818,909-933`), and the
  GatewayClass-specific raw-key union lives in `requeue_all_keys`.

## Industry comparison
controller-runtime keeps the reconciler loop generic and moves type-specific behavior to the type
(`client.Object` + per-type `Reconciler`). Status access is either typed per reconciler or goes through
the unstructured `status` subresource. It never uses a central type switch.

## Impact
This is a maintenance trap, not a current defect. The next kind with status, or the next partitioned
kind, will compile and quietly misbehave if any list is missed. The JSON round-trip on every reconcile
also adds avoidable serialization.

## Suggested direction
Introduce a small trait, for example `StatusCarrier`, in `edgion-resources` or on the processor bound. It
would provide `fn status_value(&self) -> Option<Value>`,
`fn set_status_value(&mut self, Value) -> Result<()>`, `fn current_status_slot(&mut self) -> …`,
`fn status_partitioned() -> bool`, `fn is_route()`/`policy_hooks()` with default no-op impls, and
implement it per resource. Required methods make a missing implementation a compile error. Seed status by
deserializing only the status value into the typed field, instead of round-tripping the whole object, and
remove the unknown-fields sidecar. Move the policy-ancestor and GatewayClass special cases into handler
hooks (`ProcessorHandler::after_publish` / a `requeue_all_keys` override).

## Notes
- Searched `skills/04-review/status/*` and `controller/*`. No intentional-design page covers the
  downcast dispatch. `cpu-memory/review-evidence/extract-status-json-double-serialization-cold-path.md`
  accepts serialization CPU as cold-path. This finding is about silent correctness under extension, with
  CPU as a secondary point.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P3
- `resource_processor/status_pipeline.rs:38-74`: `replace!` over 17 types, falls through to `Ok(())`.
- `status_pipeline.rs:76+`: separate `clear`/`capture`/filter lists.
- Maintenance trap, no current defect — P3 correct.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3 (unchanged)
- `resource_processor/status_pipeline.rs` and `processor.rs` unchanged since `feature-08-30`; `replace_status` still falls through to `Ok(())` for unlisted types.
- `policy_ancestor_capacity.rs` unchanged.
