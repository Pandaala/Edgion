---
name: get-raw-any-lookup-not-json-scan
description: Use when reviewing findings that mistake get_raw_any / lookup_gateway for full JSON serialization and linear scan instead of O(1) map lookup.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `get_raw_any` / `lookup_gateway` is Not JSON Serialization

**False-positive scenario**: thinking that `lookup_gateway()`, `lookup_service_cluster_ip()` call `list_json()` to fully serialize all entries to JSON and then linearly scan.

**Reality**:
```rust
// route_utils.rs
pub fn lookup_gateway(namespace: &str, name: &str) -> Option<Arc<Gateway>> {
    let processor = PROCESSOR_REGISTRY.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_raw_any(&key)?.downcast::<Gateway>().ok()
}
```
`get_raw_any` goes through `KubeRawStore` (DashMap) or `FsRawStore` (RwLock<HashMap>), both **O(1) lookups**. `downcast` only does a TypeId comparison, no JSON serialization, no linear scan.

**Verdict**: code is correct and efficient; not a performance issue.
