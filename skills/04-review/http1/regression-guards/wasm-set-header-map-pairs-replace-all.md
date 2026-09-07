---
name: wasm-set-header-map-pairs-replace-all
description: Read when a finding claims the proxy-wasm host proxy_set_header_map_pairs (HeaderOp::ReplaceAll) only sets new pairs without removing omitted headers, leaks credentials/identity headers on a whole-map replace, or collapses duplicate replacement values (e.g. multiple Set-Cookie) to the last value.
status: fixed
finding_id: runtime-03
closed: 2026-07-18
knowledge_type: regression-guards
---

# proxy-wasm `proxy_set_header_map_pairs` whole-map replace drops omitted headers and preserves duplicates (runtime-03) — FIXED

## Conclusion

`proxy_set_header_map_pairs` is a *whole-map replacement*: after it runs, the live
session header map for that map id must equal exactly the guest's new vector.
The old `HeaderOp::ReplaceAll` apply arm only iterated the new pairs and called
`set_*`, so (a) names present before the replace but omitted from the new vector
survived on the live session (a security leak — omitted `authorization`/cookie/
identity headers forwarded upstream), and (b) repeated names in the new vector
collapsed to the last value (`insert_header` replace-all). Both are fixed.

Re-reports of the "ReplaceAll leaves old headers" or "duplicate replacement
values collapse" form are **not accepted**.

## Core Rationale

**1. The op now carries the pre-replace name set.**

`proxy_set_header_map_pairs` (`edgion-gateway/src/plugins/http/wasm/host_abi.rs`)
captures the deduplicated (case-insensitive) set of header names present in the
guest snapshot *before* `*pairs = new_pairs`, and stores it in
`HeaderOp::ReplaceAll { map, remove, pairs }`. This is consistent with the live
session at apply time because prior queued ops are drained in order onto the
session before the `ReplaceAll` op, so the captured names equal the session's
names just before the replace is applied.

**2. `apply_ops` removes then re-installs.**

`apply_ops` (`edgion-gateway/src/plugins/http/wasm/plugin.rs`) first calls
`remove_*` once per name in `remove` (`remove_header` clears all values for a
name), then installs the new vector with `set_*` for the first occurrence of
each name and `append_*` for later duplicates. This makes the result a true
replacement and keeps non-combinable fields (multiple `Set-Cookie`) distinct
instead of collapsing to the last value.

**3. Same header-list family as F-001 / gateway-plugins-02.**

The set-first / append-rest duplicate handling matches the sibling fixes
[append-request-header-multi-value.md](append-request-header-multi-value.md)
(F-001) and [../csrf/set-cookie-append.md](../../csrf/regression-guards/set-cookie-append.md)
(gateway-plugins-02): `set` = `insert_header` (replace-all), `append` =
`append_header` (new field line). The wasm-specific addition is the removal of
omitted names, which the per-header `Set`/`Add`/`Remove` ops never needed.

## Fix Suggestions Not Accepted

- "Just keep `set_*` for every pair" — that is the original additive bug; omitted
  headers leak and duplicates collapse.
- "Recompute the removal set from the session at apply time" — the session
  exposes no whole-map name enumeration to the wasm drain path, and the captured
  snapshot names are already consistent (see rationale 1); carrying `remove` on
  the op is simpler and race-free.

## Re-evaluation Triggers

Re-open only if:

- The op-drain model changes so queued ops are no longer applied in order onto
  the session (the snapshot-consistency argument in rationale 1 would break).
- A session API is added that lets the drain path enumerate live header names,
  making the carried `remove` set redundant.

## Reference Cases

- runtime-03 (2026-07-18), `tasks/review-wasm-issues/`.
- Fix sites: `edgion-gateway/src/plugins/http/wasm/host_abi.rs`
  (`proxy_set_header_map_pairs`, `HeaderOp::ReplaceAll`),
  `edgion-gateway/src/plugins/http/wasm/plugin.rs` (`apply_ops`).
- Unit tests: `apply_ops_replace_all_removes_omitted_request_headers`,
  `apply_ops_replace_all_preserves_duplicate_response_values` (plugin.rs).
- Siblings: F-001 (append-request-header-multi-value.md), gateway-plugins-02
  (../csrf/set-cookie-append.md).

## Resolution references

- `runtime-03` — 2026-07-18.
