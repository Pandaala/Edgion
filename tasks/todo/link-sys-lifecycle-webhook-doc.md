# Refresh the Shared LinkSys Lifecycle Documentation

## Meta
- Status: pending
- Severity: low
- Category: documentation

## Evidence
`edgion-common/src/link_sys_core/lifecycle.rs` still states that the Gateway Webhook client implements `LinkSysLifecycle`, but the current implementation makes Webhook publication manager-owned and sequence-checked without that implementation.

## Why separate
The stale comment is in `edgion-common` and should be corrected without broad LinkSys refactoring.

## Candidate next step
Update the shared lifecycle module documentation to describe the generic provider contract and the Gateway Webhook specialization without changing runtime behavior.
