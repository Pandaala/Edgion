# Make Partial LinkSys Provider Replacement Authoritative

## Meta
- Status: pending
- Severity: medium
- Category: correctness

## Evidence
A partial update that reuses one LinkSys key while changing its provider from Webhook to another provider publishes the new provider but does not retire the old Webhook manager/runtime entry. The stale Webhook remains callable until a later authoritative full-set update.

## Why separate
The fix requires cross-provider ownership tracking and ordered cleanup semantics for every LinkSys provider, not a Webhook-only deletion in the current batch.

## Candidate next step
Define one authoritative provider identity per LinkSys key, retire the previous provider during a same-key partial replacement under the ordered transition guard, and add unit plus standalone integration coverage without modifying existing integration-test files.
