---
name: duration-schema-admission-patterns
description: Historical review evidence for Config / schema; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Duration schema admission patterns

## Recorded conclusion

`ClientTimeout`, `BackendTimeout`, `HTTPRouteTimeouts`, `HTTPRouteRetry.backoff`, `SessionPersistence` fields carry `#[schemars(pattern(...))]`; malformed values caught at admission time

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `t1-010` — —.
