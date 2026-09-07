---
knowledge_type: regression-guards
---

# Runtime-ignored fields may still have advisory diagnostics

## Conclusion

Documentation must distinguish serde/runtime behavior from the shared
unknown-field detector. A removed field can be ignored by typed deserialization
and have no compatibility shim while still being reported through Controller
warnings, `UnknownConfigFields`, diagnostics/metrics, and Admin API responses.

`successStatusCodes` is the reference case. It is ignored in favor of
`success.statusCodes`, so runtime falls back to the default "any 2xx" policy.
The resource remains admitted and synced, but the stale field is not silent:
the advisory schema walker reports its exact path. The read-only Admin validate
endpoint exposes the same finding with `ok: false`.

## Core rationale

The schema walker operates on raw input before typed round-tripping removes
unknown keys. Statements such as "the flattened serde struct ignores this
field" describe runtime compatibility only; they do not imply "no warning" or
"no log" once an ingest-side detector exists.

This distinction matters for migration guidance. Operators must know both the
effective fallback behavior and where to find the diagnostic without being
told that apply is rejected when the mechanism is intentionally warn-not-block.

## Review checklist

- Describe the runtime fallback after the stale field is dropped.
- State that apply remains admitted and `Accepted` is unchanged.
- Name the advisory status/Admin/diagnostic surfaces.
- Distinguish Admin validate's `ok: false` verdict from apply rejection.
- Do not call a field "silent" solely because serde ignores it.

## Reference case

- UFD-05 in [`review knowledge`](../../SKILL.md)
- Integration fixture:
  `EdgionPlugins_edgion-test_uf-fa.yaml`

## Resolution references

- `UFD-05` — 2026-08-22.
