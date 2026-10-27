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

`proxy_set_header_map_pairs` replaces editable fields. Request-map operations
retain their existing semantics. Response maps retain protocol-owned protected
fields even when the guest omits or attempts to overwrite them; ordinary omitted
fields are removed and valid replacement duplicates are preserved. See the
[response mutation policy](../../plugin-runtime/regression-guards/response-header-mutation-policy.md).
Exact response-map equality with an untrusted guest vector is not the contract.
The old `HeaderOp::ReplaceAll` apply arm only iterated the new pairs and called
`set_*`, so (a) names present before the replace but omitted from the new vector
survived on the live session (a security leak — omitted `authorization`/cookie/
identity headers forwarded upstream), and (b) repeated names in the new vector
collapsed to the last value (`insert_header` replace-all). Both are fixed.

A new report must identify an editable omitted field or an allowed duplicate
that is mishandled in current code. Retaining protected response fields is
intentional; historical tests are not proof of current correctness.

## Core Rationale

**1. The op now carries the pre-replace name set.**

`proxy_set_header_map_pairs` (`edgion-gateway/src/plugins/http/wasm/host_abi.rs`)
captures the deduplicated (case-insensitive) set of header names present in the
guest snapshot *before* `*pairs = new_pairs`, and stores it in
`HeaderOp::ReplaceAll { map, remove, pairs }`. This is consistent with the live
session at apply time because prior queued ops are drained in order onto the
session before the `ReplaceAll` op. Rejected protected response operations can
make the guest snapshot differ from the live map; those names are outside the
editable replacement contract.

**2. `apply_ops` removes then re-installs.**

`apply_ops` (`edgion-gateway/src/plugins/http/wasm/plugin.rs`) first calls
`remove_*` once per name in `remove` (`remove_header` clears all values for a
name), subject to response policy, then installs the allowed new vector with `set_*` for the first occurrence of
each name and `append_*` for later duplicates. This replaces the editable fields and keeps non-combinable fields (multiple `Set-Cookie`) distinct
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

Re-evaluate if:

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

- `unify-response-header-mutation-policy` — response replace-all retains protected fields; current verification is recorded in the shared policy page.

## Detached deduplication ownership decision (wasm-01)

Keep the consuming ReplaceAll loop and its compact owned lowercase seen keys.
A disposable source-derived replay probe compared the current loop with reusing
already-lowercase exact-capacity names after writer calls. Writer transcript parity
covered duplicates, mixed/spare names, protected response fields, first-writer
failure and unknown maps. This candidate saved allocations, but its lowercase scan
and contains-plus-insert work regressed 256-byte unique-name fixtures in two runs:
about 3.90 to 5.21 microseconds in one run, with roughly 34–38 percent regressions
across both. Allocation events fell from 77 to 61 in that fixture. Duplicate-heavy
fixtures improved about 13 percent; those gains do not erase the relevant regression.
A separately reviewed HashMap<String, ()> Entry alternative then removed the
extra lookup and inserted before the writer. It preserved transcripts and early
source-value release, but two runs still regressed long-name replay about 20–24
percent (3.58 to 4.44 and 3.80 to 4.55 microseconds). It retained the 77-to-61
allocation reduction, improved duplicate-heavy fixtures about 9–14 percent, and
was about 5–8 percent slower on mixed-case fixtures. Its measured allocation bytes
included bucket storage; unit values were not assumed free. Both consuming reuse
alternatives are therefore rejected for this scope. No production replay or ABI
representation changed.

A borrowed-Cow contrast also demonstrated why avoiding key copies alone is not
sufficient. A separate source-value lifetime audit of 16 values of 8,192 bytes
observed 8,192 source bytes still live at the last writer for all consuming loops,
versus all 131,072 bytes for the borrowed loop. All released their source values by
loop end. Detached writer outputs and key capacity were recorded separately;
these logical lifetime observations are not allocator RSS or complete peak-memory
measurements. Timings include a synthetic recording writer and complete replay/
drop, not a real VM, session transport or whole-callback throughput benchmark.

Original spelling, removals first, protected skipping before deduplication and
first-set/later-append semantics remain unchanged, including marking the first
occurrence seen when its writer returns an error. Re-evaluate with representative
workload evidence or a separately reviewed dedup design that preserves early value
release without a meaningful regression. This is not a general prohibition on
owned-name reuse. Snapshot/pending sharing remains separately constrained by the
[ABI retention decision](../../cpu-memory/regression-guards/wasm-abi-retention-budget.md).
