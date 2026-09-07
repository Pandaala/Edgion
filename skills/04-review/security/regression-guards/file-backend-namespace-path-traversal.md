---
name: file-backend-namespace-path-traversal
description: Use when reviewing FileSystemStorage / build_resource_path findings claiming the namespaced admin API lets a {namespace}/{name} segment escape the conf root via "..".
knowledge_type: regression-guards
---

# File-backend `namespace` path traversal — FIXED (fr-csync-01, 2026-05-29)

## Conclusion

The `{namespace}` (and `{name}` / `{kind}`) segments reaching `FileSystemStorage`
can no longer escape the conf root. Re-reports of the form "`build_resource_path`
joins a raw `namespace` subdir, so `namespace=\"..\"` writes/reads/deletes a
`.yaml` one level above the conf root" are **not accepted** — verify the
`validate_path_components` guard is still wired into the `CenterApi` methods
before re-reporting.

## Core Rationale

**1. Where the guard lives**

`edgion-controller/src/conf_mgr/conf_center/file_system/storage.rs` defines
`validate_path_components(kind, namespace, name)`, which rejects any component
containing `..`, `/`, or `\` with `ConfWriterError::ValidationError` (mapped to
HTTP `400 BAD_REQUEST` in `controller/api/common.rs`). It is called at the top of
every `CenterApi` method that turns an HTTP-supplied identifier into a path:
`set_one`, `create_one`, `update_one`, `get_one`, `delete_one`. This is the
chokepoint where untrusted input enters the storage backend, so all four
mutating handlers plus `get_namespaced` are covered.

**2. Why not change `build_resource_path` to return `Result`**

`build_resource_path` is kept infallible on purpose: making it fallible would
ripple a signature change to `build_resource_path_from_key`, `resolve_path`,
`status.rs`, and `resource_controller.rs` — all of which derive their key from
**filenames already on disk** (the file-watcher / status path), not from HTTP
input. Those are not an attack surface (issue Notes), so guarding at the
HTTP-input boundary is both sufficient and minimal.

**3. Why these characters are safe to reject**

Legitimate Kubernetes identifiers are RFC 1123 labels and never contain `..`,
`/`, or `\`. axum binds `{namespace}` as a single percent-decoded URL segment
(it cannot contain a literal `/`, and `../../` cannot be captured), so a bare
`..` was the only traversal vector and it is now rejected outright. `name` was
never a traversal vector (it is only interpolated into the filename, never
`root.join(name)`) but is validated too for defense in depth.

## Fix Suggestions Not Accepted

- "Add the guard only at the handler layer (`namespaced_handlers.rs`)" — narrower
  than the storage boundary; a future non-HTTP caller of `CenterApi` would bypass it.
- "Make `build_resource_path` return `Result` and canonicalize" — unnecessary
  signature ripple to the disk-derived `from_key` path (not an attack surface);
  rejected for blast radius.
- "Re-report as still vulnerable" — verify `validate_path_components` is present
  and called from the five `CenterApi` methods first.

## Re-evaluation Triggers

Re-open only if:

- A new `CenterApi` method that constructs a path from HTTP input is added
  without calling `validate_path_components`.
- The route layer starts accepting multi-segment `{namespace}` (would raise the
  traversal depth beyond one level).

## Reference Cases

- `tasks/bug/fr-csync-01-file-backend-namespace-path-traversal.md` (closed 2026-05-29).
- `edgion-controller/src/conf_mgr/conf_center/file_system/storage.rs`
  (`validate_path_components`, `path_tests`).
- `edgion-controller/src/api/common.rs` (`ValidationError` → 400).

## Resolution references

- `fr-csync-01` — 2026-05-29.
