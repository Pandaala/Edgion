# D0078 — Outbound Warn-if-missing Diagnostics

## Meta
- Status: needs-implementation
- Severity: low
- Discovered in: B0116 WebhookKeyGet facade analysis
- Category: observability-contract
- Canonical task: [`tasks/todo/outbound-warn-if-missing-diagnostics.md`](../../../todo/outbound-warn-if-missing-diagnostics.md)

## Evidence
The shared outbound request builder currently handles `PresencePolicy::WarnIfMissing` together with `SkipIfMissing`, silently omitting both. Resource schema, architecture, operator documentation, and an integration fixture promise that the warning variant continues while adding a PluginLog alert. Both WebhookKeyGet and ForwardAuth use this shared builder.

## Why deferred
The fix belongs to the shared outbound mechanism and must return bounded, value-free diagnostic evidence to both consumers without adding tracing, exposing field values, or changing their failure behavior. The B0116 facade owns none of this behavior.

## Integration-test impact
Existing frozen coverage already proves omission and continued execution but does not assert the alert. Focused shared-builder and consumer unit tests can prove the diagnostic contract, so no integration-test edit or separate integration issue is needed.
