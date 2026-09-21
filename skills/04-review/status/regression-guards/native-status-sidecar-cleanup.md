---
name: native-status-sidecar-cleanup
description: Use when reviewing how FileSystem/etcd status persistence clears an error sidecar after a resource is repaired.
knowledge_type: regression-guards
---

# Repaired resources must not keep reporting their previous failure

## Conclusion and scope

In FileSystem and etcd modes, a parse failure or identity mismatch persists an
`ErrorStatus` document beside the resource (`<file>.yaml.status`, or the etcd
`/status/` key). Once the resource is repaired, that document must be removed
whenever this round computes no status of its own; a resource that does compute
one overwrites it as before.

The clearing must run on every status-persisting entry point of both backends:
`InitApply`, an `Apply` received before `InitDone` (the lag/recovery path), and
the workqueue worker — six sites in total. An init path is the common trigger,
because a resource repaired while the controller was down is applied through
init, not through a runtime event.

Deletion is admitted only for a document this controller wrote as an error
sidecar. `ERROR_STATUS_CONDITION_TYPE` (`Ready`) is the sole marker: no resource
handler manages a condition of that type. Any other persisted status — including
a Kubernetes controller envelope imported into a native backend — is left in
place even when the current round persists nothing.

## Rationale and evidence

`ResourceProcessor::process_work_item` returns
`status_changed = status_changed && persisted_status.is_some()`, so a resource
that computes no status can never report a status change. Gating the cleanup on
`status_changed` therefore made it unreachable for exactly the resources that
need it. Handlers for `Secret`, `ConfigMap`, `Service` and `ReferenceGrant`
(registered in `conf_center/mod.rs` `for_each_provider_core_resource!`) do not
implement `update_status`, so their sidecars survived every successful reapply
and `cleanup_orphans()` could not remove them either — the configuration file
still exists, so they are not orphans.

The correct predicate is `persisted_status.is_none()` plus the sidecar marker.
`persisted_status.is_none()` alone is not safe: `seed_existing_status` skips only
error sidecars, but `status_pipeline::has_non_native_status` clears an imported
Kubernetes envelope from the object in native mode, which also yields
`None` while the persisted document is legitimate.

The marker's consequence is widened by this rule. Before it, a false positive
only skipped seeding; now it also authorizes a delete. No handler writes a
`Ready` condition today (handlers use `Accepted`, `Programmed`, `ResolvedRefs`,
`PartiallyInvalid`, `Conflicted`; `EdgionAcme` uses `Ready` only as
`status.phase`), so the marker stays exclusive — but it must be kept that way.

Canonical authority: [FileSystem configuration center](../../../01-architecture/01-controller/03-config-center/01-file-system.md)
and [status management](../../../01-architecture/01-controller/10-status-management.md).
The marker and its shared predicate live in `conf_center/common.rs`
(`ERROR_STATUS_CONDITION_TYPE`, `is_error_status_sidecar`,
`is_error_status_sidecar_json`); `seed_existing_status` and all four cleanup
sites consume that one implementation.

## Alternatives and limits

Deleting whenever the round persists nothing was rejected: it destroys an
imported controller envelope, proven by
`imported_controller_envelope_status_is_never_cleared`. Deleting unconditionally
after a successful reconcile was rejected for the same reason and additionally
costs a syscall per work item — an etcd RPC per event. Reading the persisted
status specifically for the cleanup was unnecessary: both backends already read
it for change detection, so the marker is computed from that value before it is
consumed.

The `Skipped` arm still leaves an orphan sidecar for a key that is neither
cached nor stored; that residual is unchanged and covered by
`never_cached_key_delete_is_skipped_without_status_write`.

Two further residuals are accepted rather than mechanised. `persisted_status` is
also forced to `None` when `persistence_allowed` is false, so a round that
deliberately skips persistence still clears the sidecar; the blast radius is one
document this controller wrote, and suppressing it would require threading the
persistence decision out of the processor. And a spec file that embeds its own
`status:` block which the handler recomputes identically yields
`status_changed == false` with `persisted_status.is_some()`, so a coexisting
sidecar is not cleared; that shape is pre-existing and outside this rule.

In both workers, the `status_changed` arm whose `extract_status_value` returns
`None` now warns instead of deleting. That arm is unreachable — `status_changed`
implies `persisted_status.is_some()`, and the returned object is cloned after
the status is replaced — and warning matches what the init paths already did.

## Verification and re-evaluation

Fresh verification on 2026-09-21:

- `valid_to_malformed_to_repair_revokes_then_recovers_via_worker` and
  `valid_to_identity_mismatch_to_repair_revokes_then_recovers_via_worker` now
  pass; both failed before this correction.
- New regressions: `init_clears_error_status_of_a_resource_repaired_while_down`,
  `handler_owned_status_survives_a_statusless_reconcile`,
  `imported_controller_envelope_status_is_never_cleared`, and the
  `error_status_tests` predicate cases.
- Mutation check: dropping the marker from the predicate — which makes the
  cleanup unconditional for a statusless round — fails both
  `imported_controller_envelope_status_is_never_cleared` and
  `handler_owned_status_survives_a_statusless_reconcile`. That is one mutation
  with two witnesses, not two independent mutations. The marker's positive
  direction is covered only by the two originally-failing repair tests.
- Controller `conf_center` suite: 257 passed. The two
  `unknown_fields_seam_tests` LinkSys failures are pre-existing, reproduced on a
  stashed tree, and tracked as `linksys-timeout-advisory-test-fixtures`.

Verification limit: the etcd sites carry no unit test, because
`EtcdStatusHandler` requires a live `EtcdLinkClient`. They are code-symmetric
with the FileSystem sites and share the one predicate; a live-etcd or
integration regression would be the next strengthening step.

Re-evaluate if a native backend gains a second status-writing entry point, if
`ERROR_STATUS_CONDITION_TYPE` stops being exclusive to `ErrorStatus`, or if a
handler starts managing a `Ready` condition.

## Resolution references

- `filesystem-repaired-resource-error-status` — investigation opened 2026-09-21
  from the `gatewayclass-status-reporting-ownership` verification run; confirmed
  as a real defect and corrected across both native backends.
