# Retain EdgionPlugins Last-known-good Only for the Same Resource Identity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Bind EdgionPlugins DSL/WAF last-known-good retention to resource identity rather than only `namespace/name`. Partial add/update history is now identity-safe, but a reconnect full snapshot cannot distinguish an invalid update from deletion followed by creation of a different object at the same key.

## Requirements

- Retain a valid compiled plugin generation for an invalid replacement only when the replacement is the same logical resource.
- A recreated resource with a different Kubernetes UID must replace the previous runtime and remain nonactivatable/fail-closed when invalid.
- Define conservative, deterministic identity rules for FileSystem and etcd inputs, where a Kubernetes UID is unavailable.
- Preserve identity across the source, event compression, Controller-to-Gateway conf-sync, and Gateway full-snapshot publication path.
- Keep partial delete-to-add and reconnect full-snapshot behavior equivalent.
- Reuse a shared cross-resource identity contract with the TLS identity work where its semantics genuinely match; do not add a plugin-local UID shortcut.
- Preserve plugin stage validation, DSL/WAF compilation parity, canonical access-log ownership, redaction, failure policy, and lock-free Gateway reads.

## Scope

### In scope

- EdgionPlugins generation identity metadata and full-snapshot LKG decisions.
- Kubernetes, FileSystem, and etcd identity semantics.
- Conf-sync inputs needed to preserve identity without exposing sensitive configuration.
- Focused source, compression, store, and reconnect tests.

### Out of scope

- Changing ordinary plugin configuration or stage semantics.
- Retaining non-DSL/WAF invalid generations that are currently fail-closed.
- Request-pinned plugin snapshots or an access-log ownership redesign.
- A general conf-sync protocol redesign unrelated to resource identity.

## Acceptance criteria

- An invalid update with the same Kubernetes UID retains the original compiled `Arc<CompiledPlugins>`.
- A delete/recreate with a different UID never serves the deleted runtime and publishes a nonactivatable generation when the replacement is invalid.
- A reconnect full snapshot produces the same result as the equivalent partial update or delete/add history.
- FileSystem and etcd inputs have documented conservative identities and matching tests; missing identity never silently authorizes retention across suspected recreation.
- A later valid generation recovers normally without weakening validation or failure policy.
- Gateway reads remain lock-free and perform no compilation, parsing, or tracing.
- Targeted plugin/conf-sync tests, workspace check, Clippy, and repository guards pass.

## Related work

- [`Retain TLS last-known-good only for the same resource identity`](../tls-last-known-good-resource-identity.md)
- [`Add EdgionPlugins LKG identity integration coverage`](../add-edgionplugins-lkg-identity-integration-coverage.md)

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Fix explicit partial add/update semantics in B0062 and defer full-snapshot identity. | Compressed add identifies a new generation, while reconnect full snapshots require one cross-source identity contract. |
