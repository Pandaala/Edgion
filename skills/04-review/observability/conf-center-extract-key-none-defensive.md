---
name: conf-center-extract-key-none-defensive
description: Use when reviewing findings that request log/metric instrumentation on the `None` branch of `extract_key_from_path` / `extract_key_from_etcd_key`; explains the upstream watcher pre-filter that makes those branches unreachable in production.
---

# `extract_key_from_path` / `extract_key_from_etcd_key` None Branches Are Defensive Fallbacks

## Conclusion

Findings of the form **"`extract_key_from_path` / `extract_key_from_etcd_key` returns `None` silently on malformed key — no log, no metric, ops cannot detect illegal files"** are not accepted. The cited None branches are unreachable in production:

- The malformed-name path is filtered **upstream at the watcher layer**, not inside the per-kind `resource_controller`.
- Parse failures that legitimately reach the controller already emit `tracing::warn!` with `path` / `key` / `kind` / `error`; these are not silent.
- Adding `tracing::debug!` + a counter inside the unreachable None branch produces a metric that essentially never increments and a log line that duplicates the upstream warn.

## Architectural Evidence

### File system backend

The watcher pre-filters before any controller sees the event:

- Init scan — `src/core/controller/conf_mgr/conf_center/file_system/file_watcher.rs:186` calls `parse_resource_filename`; non-conforming names (e.g. `my-route.yaml`, `kustomization.yaml`) are dropped at `tracing::trace!` before being grouped by kind.
- Runtime dispatch — `file_watcher.rs:435` does the same in `dispatch_file_event` and additionally routes to a per-kind sender by `info.kind` (line 447). A file delivered to the `HTTPRoute` controller is therefore guaranteed to be `HTTPRoute_*_*.yaml`.

By the time `extract_key_from_path(&path, expected_kind)` runs in `resource_controller.rs`, both `parts.len() == 3` and `parts[0] == expected_kind` hold by construction. The None branch survives only as a defensive fallback against future refactors.

### Etcd backend

The etcd watcher subscribes by prefix `{prefix}/conf/{kind}/...`; malformed keys cannot enter the event stream. The init/apply parse-error sites that call `extract_key_from_etcd_key` already emit `tracing::warn!` with the full `key` (`etcd/resource_controller.rs:143-149`, `199-205`); the extract is only used to locate the error-status file, and is not the operator-facing signal.

## What an Operator Actually Sees When Dropping a Misnamed File

1. File watcher sees the new file, runs `is_yaml_file` (passes), then `parse_resource_filename` (fails) → `tracing::trace!` and drop. **This is the actual visibility surface.**
2. No event is dispatched to any per-kind controller, so `extract_key_from_path` is never invoked.

If a future review wants to improve operator UX for this scenario, the right edit is to **upgrade the trace! at `file_watcher.rs:187` and `:436` to debug! or info!** — not to add observability inside the unreachable None branch of `extract_key_from_*`.

## Decision Rule

When a review reports "extract_key_* returns None silently, add log/metric":

1. Verify the call site — if it lives in `conf_mgr/conf_center/{file_system,etcd}/resource_controller.rs`, the None branch is unreachable per the architecture above.
2. Verify whether the parent context already logs (file_system parse-error sites at `resource_controller.rs:226-232` and `340-346`; etcd at `:143-149`, `:199-205`). If yes, the "silent" claim is false.
3. Close the finding by linking to this file. Re-route the operator-visibility concern to the watcher layer if the reporter cares about misnamed-file feedback.

## Background

- Decision date: 2026-05-16
- Source finding: `P3-c3a-018` (cited file:line `file_system/resource_controller.rs:637-658` was already stale; the current function lives at `:671`).
- Related files:
  - `src/core/controller/conf_mgr/conf_center/file_system/file_watcher.rs` (filtering layer)
  - `src/core/controller/conf_mgr/conf_center/file_system/resource_controller.rs::extract_key_from_path`
  - `src/core/controller/conf_mgr/conf_center/etcd/resource_controller.rs::extract_key_from_etcd_key`
