# D0020 — Refresh the Shared LinkSys Lifecycle Documentation

## Meta
- Status: needs-implementation
- Severity: low
- Discovered in: B0053, Webhook lifecycle specialization
- Category: documentation

## Evidence
`edgion-common/src/link_sys_core/lifecycle.rs` still states that the Gateway Webhook client implements `LinkSysLifecycle`, but B0053 intentionally removes that implementation and makes Webhook publication manager-owned and sequence-checked.

## Why deferred
The stale comment is in `edgion-common`, outside the active Gateway anchor and B0053's exact support allow-list.

## Candidate next step
Update the shared lifecycle module documentation to describe the generic provider contract and the Gateway Webhook specialization without changing runtime behavior.
