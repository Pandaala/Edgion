---
name: shared-service-backend-ref-predicate
description: Historical review evidence for Redundancy / L4 routing; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Shared service backend ref predicate

## Recorded conclusion

The `is_service_backend_ref` predicate (a BackendRef targets a core Service when group is `""`/`"core"`/omitted and kind is `"Service"`/omitted) was duplicated across five L4 route modules with two signatures — struct-based in `{tcp,tls,udp}/conf_handler_impl.rs`, raw-field in `tls/{proxy,passthrough_proxy}.rs` — with no compiler-enforced link. Consolidated into a single `pub(crate) fn is_core_service_backend_ref(group: Option<&str>, kind: Option<&str>)` in `routes/common.rs`; all five sites now call it (struct callers pass `br.group.as_deref(), br.kind.as_deref()`), and the three per-module unit tests were merged into one shared test in `common.rs`. Pure refactor, zero behavior change (filter semantics bit-identical: `None`/`""`/`"core"` group, `None`/`"Service"` kind). Re-reports of the five-copy duplication form are not accepted; verify `routes/common.rs::is_core_service_backend_ref` is the single definition before re-reporting.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `path-norm-F006` — 2026-05-26.
