# Remove the Stale Gateway Admin Redaction Design Link

## Meta

- Status: pending
- Type: docs
- Priority: P3
- Discovered in: Gateway quality campaign B0003

## Evidence

`skills/04-review/admin-api/mandatory-design/gateway-admin-configclient-redaction.md` links to the removed file `docs/superpowers/specs/2026-06-07-gateway-admin-api-harden-design.md`.

## Why deferred

The missing historical design link does not affect the Admin bearer boundary or its runtime behavior, and correcting unrelated provenance would widen B0003.

## Candidate next step

Remove or replace the stale provenance link after confirming whether an archived canonical design record exists.
