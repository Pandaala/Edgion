---
name: resource-change-notification-backpressure
description: Historical review evidence for ACME; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Resource change notification backpressure

## Recorded conclusion

`notify_resource_changed` switched from `try_send` (silent drop on `Full`) to `async` + `tx.send(...).await` (back-pressure instead of losing the `ResourceChanged` event), mirroring `Workqueue::enqueue`, and now returns `bool`; the admin `POST .../trigger` endpoint returns `503` + `success:false` ("ACME service not running; trigger for `<key>` was not queued") when the service is not running instead of a misleading `200`. Awaiting is safe — `get_sender()` releases the global lock before the await (no sync lock held across the suspension point), and the worker does one K8s `get` per command then `tokio::spawn`s the ACME flow, so the bounded channel drains fast and cannot stall the processor. **NB:** an earlier 2026-05-25 record marked this fixed, but the change was never actually applied in code (no commit on any branch carried it) — it was genuinely landed on branch `feature-05-22` on 2026-06-14. Re-reports of the `try_send` silent-drop form are not accepted

## Evidence boundary

The resource handler now invokes this bounded notification from `after_publish`,
not `parse`, after comparing against the previous processed cache. This ordering
and deduplication correction is covered by the [notification guard](resource-change-notification-gating.md);
the async send, startup scan, and explicit Admin trigger contracts remain unchanged.

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `controller-03` — 2026-06-14.
