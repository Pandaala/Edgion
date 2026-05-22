---
name: extract-status-json-double-serialization-cold-path
description: Use when reviewing findings about extract_status_json being called twice in process_resource_inner causing repeated serialization; it is a cold path with unavoidable API-shape constraints.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `extract_status_json` Double Serialization is Cold Path + API Shape

**False-positive scenario**: `processor.rs:process_resource_inner` calls `extract_status_json(&parsed_obj)` once before and once after `update_status()`; each call internally `serde_json::to_value(obj)` fully serializes the entire object then `.get("status")`; flagged as "repeated serialization; CPU rises noticeably for HTTPRoute with many rules".

**Reality**:

- **Cold path**: `process_resource_inner` is only triggered by K8s resource watch events; not the request path. Watch event frequency is far below request frequency, and is queued asynchronously anyway.
- **The second serialization is unavoidable**: between the two calls, `self.handler.update_status(&mut parsed_obj, ...)` modifies status; the first `Value` cannot be reused. The "Approach A: serialize once and reuse the Value" in the report does not hold semantically.
- **What can really be saved is "serialize the entire object" → "serialize only the status subtree"**, but `extract_status_json<T: Serialize>` is generic and only requires `Serialize`; cannot directly access the `.status` field. To optimize, a `status_value() -> Option<Value>` method must be added to the K resource trait (affects trait definition + all impls); or each status type must implement `PartialEq` and `status_semantically_changed` must be rewritten (currently relies on JSON comparison to ignore non-semantic fields like `lastTransitionTime`; rewriting to struct-level comparison has even larger change surface).
- The comparison function `status_diff::status_semantically_changed(kind, old, new)` currently takes JSON strings; eliminating serialization cascades to its semantic comparison logic.

**Verdict**: cold path + API shape; the change surface (trait extension / comparison-semantics rewrite) far exceeds the benefit. Any "`extract_status_json` repeated serialization" finding is closed per this entry, unless an actual profile shows that controller CPU time is significantly spent in this function.
