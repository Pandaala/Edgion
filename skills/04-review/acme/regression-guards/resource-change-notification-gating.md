---
name: resource-change-notification-gating
description: Historical review evidence for ACME; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Resource change notification gating

## Recorded conclusion

`EdgionAcmeHandler::parse` now gates `notify_resource_changed` on `spec_changed \|\| creds_changed`; restart resilience via `scan_all_acme_resources`

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `c1-013` — —.
