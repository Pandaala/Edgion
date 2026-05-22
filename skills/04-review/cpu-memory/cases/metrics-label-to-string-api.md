---
name: metrics-label-to-string-api
description: Use when reviewing findings that criticize .to_string() in metrics recording functions as causing unnecessary heap allocation per call.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# metrics label `.to_string()` is an API Hard Requirement

**False-positive scenario**: criticizing `.to_string()` in `record_backend_request()` / `record_request_duration()` as causing unnecessary heap allocation per call.

**Reality**: `metrics` 0.24's label value type is `SharedString = Cow<'static, str>`. Dynamic values (gateway name, namespace, route, etc., which are not `'static`) **must** be `Cow::Owned(String)`; `.to_string()` is the minimal API-satisfying form. The `Arc<str>` and `&str` approaches are infeasible because their lifetimes do not satisfy `'static`. The main cost of metrics calls comes from registry hash + atomic operations, not the string allocation itself.

**Verdict**: not a performance issue; no fix needed.
