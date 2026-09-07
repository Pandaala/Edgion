---
name: iprefs-status-diagnostic-already-neutral
description: Use when reviewing claims that non-RequestAccessPolicy Condition ipRefs failures expose RequestAccessPolicy-specific ResolvedRefs messages. The resolver warning and typed status-classification paths are distinct.
knowledge_type: review-evidence
---

# Non-RAP `ipRefs` status diagnostics are already neutral — FALSE POSITIVE (cl-06)

## Conclusion

The cl-06 claim was independently traced and closed as a false positive on
2026-08-24. `lookup_ip_list` contains `RequestAccessPolicy IP data ref ...`
warning text, but that text is not the `ResolvedRefs` message for entry-level,
body, dye, or RequestRestriction conditions.

## Two diagnostic channels

`condition_tree::resolve_ip_match` receives `warnings=None` for operator-authored
entry, body, dye, and RequestRestriction conditions. A lookup failure marks their
`resolved_ips` unresolved, but the returned warning string is not retained.

`EdgionPluginsHandler::update_status` separately calls `resolve_ip_list_refs`.
That read-only walker visits the fail-closed Condition surfaces and classifies an
unavailable target as `ResolvedRefsError::EdgionConfigDataNotFound`. The resulting
status message is neutral, for example:

```text
EdgionConfigData 'default/missing-nets' (expected type=IpList) not found
```

RequestAccessPolicy resolved profiles are the only callers that pass
`Some(&mut config.resolution_warnings)` to `resolve_ip_match`. The
RequestAccessPolicy-specific lookup text is therefore visible only in a
RequestAccessPolicy warning, where that ownership label is accurate.

## Review guard

Do not infer a resource status message from a low-level resolver's returned
string. Trace whether the caller retains the string, and then inspect the typed
status classifier used by `update_status`. Re-open only if a non-RAP caller begins
persisting `lookup_ip_list` warnings directly, or bypasses `resolve_ip_list_refs`.

The existing `condition_tree_missing_ip_ref_sets_resolved_refs_false` test covers
the RequestRestriction polarity and the presence of `ResolvedRefs=False`.

## Resolution references

- `cl-06` — 2026-08-24 (❌ false positive).
