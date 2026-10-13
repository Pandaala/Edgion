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

For direct Kubernetes, filesystem, and etcd ingest, the finding can populate
status and diagnostics. Admin writes return the warning from the submitted
document, then persist a typed round-trip that drops the unknown key; later
status and diagnostics do not retain that Admin-submitted finding.

## Core rationale

The schema walker operates on raw input before typed round-tripping removes
unknown keys. Statements such as "the flattened serde struct ignores this
field" describe runtime compatibility only; they do not imply "no warning" or
"no log" once an ingest-side detector exists.

This distinction matters for migration guidance. Operators must know both the
effective fallback behavior and where to find the diagnostic without being
told that apply is rejected when the mechanism is intentionally warn-not-block.

## Call-local YAML preparation

ARCH-091 gives `conf_center/common.rs::parse_resource_content_inner` one raw YAML
view per invocation. Source-GVK checks and source selection borrow that view;
selected alpha conversion consumes it after the existing schema check. Advisory
unknown-field projection borrows the same raw view after typed decoding, so
unknown keys are not lost through a typed round-trip. Other branches drop the
view before original-text typed decoding. No view survives the call or enters a
resource, cache or transport object.

The standalone API-version validator still parses and validates its own input.
Normal and beta source decoding still use original text; malformed YAML and raw
source identity errors retain precedence over typed failures. The diagnostic
helper's JSON conversion remains non-fatal and leaves existing findings unchanged
on failure. Shared parser tests preserve adapter/metadata/status behavior, clean
and typo paths, malformed/duplicate/tagged-text controls and error locations.
The direct non-string-key conversion test is defensive seam coverage, not a claim
that the same malformed key is accepted by the typed ingress path.

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
