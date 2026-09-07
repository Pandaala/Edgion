---
name: update-namespaced-reject-body-name-mismatch
description: Use when reviewing controller Admin API update_namespaced / update_cluster body metadata.name-vs-URL-path mismatch findings — both handlers now reject present-and-mismatched names with 400; absent name defers to URL.
knowledge_type: regression-guards
---

# Controller Admin API update_namespaced / update_cluster Trusts Body metadata.name

**Status / verdict**: fixed (controller-02, 2026-05-24)

When reviewing controller Admin API findings of the form "`update_namespaced` / `update_cluster` persist the body's `metadata.name` without checking the URL path name / a PUT whose body claims a different name desyncs storage path vs. content / non-K8s (FileSystem/Etcd) resource stored at URL name but content says another name / processor restart emits duplicates": **fixed (controller-02, 2026-05-24)**. Both handlers (`edgion-controller/src/api/namespaced_handlers.rs`, `cluster_handlers.rs`) now reject with `400 BAD_REQUEST` when the body `metadata.name` (and, for namespaced, `metadata.namespace`) is **present and != the URL value**, via a single `extract_resource_metadata` guard before the per-kind `match` (the helper `create_namespaced` already uses). Absent body name/namespace defers to the authoritative URL value — so this is **present-and-mismatched only**, not "require body name", to avoid breaking clients that omit it. Aligns with K8s API server semantics. Note: Admin REST API RFC/REST-convention findings remain out of scope (see the admin-endpoint entry above) — this was closed as a storage/cache **correctness** fix, not a REST-status-code conformance change. Re-reports of the trust-body-name form are not accepted; verify the current guard before re-reporting. Unit regressions live in `update_identity_tests` in both handler files.

## Resolution references

- `controller-02` — 2026-05-24.
- `v1-01` — 2026-05-29.
