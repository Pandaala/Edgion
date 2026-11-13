---
name: trailer-mutation-header-map-capacity
description: Use when reviewing ExtProc response-trailer mutation application, HeaderMap capacity, and processor-controlled header-name floods.
knowledge_type: regression-guards
---

# ExtProc trailer mutation never panics on HeaderMap capacity

## Conclusion and scope

`apply_mutation` in `edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs`
applies every processor `set_headers` entry to a cloned candidate trailer map with
the fallible `http::HeaderMap::try_append` / `try_insert`. `MaxSizeReached` becomes
`Err(())`, which the caller turns into the existing `bad-verdict` outcome under
`failureModeAllow`. The original trailers stay unchanged, as for every other
rejected verdict.

This covers only the ExtProc response-trailer mutation map. The Pingora
`insert_header_value` wrapper and Wasm callout header maps are separate roots
(findings wasm-01 and wasm-03 of the 2026-11-08 Gateway panic audit). wasm-01 is
bounded on the Edgion side by the [ExtProc header mutation set cap](header-mutation-operation-cap.md)
and the [Wasm per-phase header write cap](../../wasm/regression-guards/header-write-cap-per-phase.md);
wasm-03 is still tracked in `tasks/panic-issues/`.

## Rationale and evidence

`http` 1.5.0 `HeaderMap::append` and `insert` call
`try_*(..).expect("size overflows MAX_SIZE")` (`src/header/map.rs:1289`, `:1433`).
A map holds at most `usable_capacity(1 << 15) = 24,576` distinct names. Growing
past that returns `MaxSizeReached` from `try_reserve_one` / `try_grow`. That number
is an upper bound: a Yellow (displacement-danger) map at raw capacity 32,768 with at
least 6,554 entries also fails to grow. The fix handles both cases the same way.

The 256-field and 64 KiB trailer caps run in `validate_trailer_map` only after the
whole mutation has been applied. Removals follow all sets, so an intermediate map
above the cap can still produce a valid result. New names are admitted
unconditionally on H2, subrequest, and custom downstreams
(`downstream_response_trailer_name_allowed`). The ExtProc tonic client keeps the
4 MiB default decode limit. As a result, a buggy or malicious processor could send
about 25,000 distinct names (about 375 KB) and hit the panicking call. This
includes the default `APPEND_IF_EXISTS_OR_ADD` action, because this local map does
not go through the hardened Pingora `append_header_value`.

Before the fix, the panic unwound the request task after the body had been sent.
The release profile does not use `panic = "abort"`, so the process survived. The
Edgion Pingora fork spawns one task per H2 stream: `pingora-core/src/apps/mod.rs`
passes a spawning closure to `accept_downstream_sessions`
(`pingora-core/src/protocols/http/v2/server.rs`). On H2 the panic therefore reset only that stream,
not its siblings. The finding's claim that every multiplexed stream was reset does
not match this code. On HTTP/1 the connection closed, but there a new name must
already be declared in the committed `Trailer` header.

## Alternatives and limits

The finding also suggested two early-rejection rules. Both were rejected because
they refuse mutations whose final map is valid:

- Rejecting when `set_headers.len() + remove_headers.len()` exceeds
  `TRAILER_FIELD_CAP`. Example: 200 upstream trailers, 150 `OverwriteIfExists`
  sets and 150 removals. That is 300 operations, but the result has 50 fields.
- Rejecting when the intermediate `candidate.len()` would exceed the cap before an
  add. Example: 200 upstream trailers, 100 new names and 100 removals. The map
  reaches 300 fields mid-mutation, but the result has 200 fields.

The fallible APIs change only the overflow outcome, from a panic to a rejected
verdict.

The panic is gone, but the CPU work is not yet bounded. Processor input can still
build up to 24,576 distinct names before the final validation rejects the map, or
an unbounded number of repeated values for one name (`extra_values` has no
`MAX_SIZE` check), all within the 4 MiB gRPC message. Green-state maps hash with
unkeyed FNV truncated to 15 bits. The vacant-slot branch of `insert_phase_one`
ignores the displacement-danger flag. Names crafted to share one hash therefore
never switch the map to SipHash, as long as no existing entry sits in the cluster's
probe path. The processor sees the upstream trailer names, so it can choose a hash
that avoids them. That can make this pre-validation work quadratic on a
Tokio worker. This conclusion comes from reading the code and has not been
measured. It predates wasm-02 and the fix does not make it worse. The follow-up is
`tasks/todo/ext-proc-trailer-mutation-collision-cpu-bound.md`. Do not treat this
page as accepting that cost.

## Verification and re-evaluation

Verified on 2026-11-12 with
`cargo test -p edgion-gateway --lib ext_proc::response_trailer` (22 passed). Two
new unit tests cover the fix:

- `distinct_name_flood_beyond_header_map_max_size_is_rejected_without_panic`
  sends 25,000 distinct names through `apply_mutation` for each adding action.
- `distinct_name_flood_follows_bad_verdict_failure_policy` drives the full
  trailer callback and checks the fail-open and fail-close `bad-verdict` outcomes
  and that the original map is preserved.

Both tests panicked at `http-1.5.0/src/header/map.rs:1433` when run against the
pre-fix source. No integration run was added: the scenario needs a hostile
processor, and the `bad-verdict` failure-policy path is pre-existing.

Re-evaluate if any of these change:

- trailer mutation stops using a cloned `http::HeaderMap`;
- the field cap moves ahead of application;
- a new `HeaderMap` write path on processor input appears (for example `entry` or
  `extend`), since those also panic on `MAX_SIZE`;
- `http` is upgraded past a change to the `try_append` / `try_insert` contract.

## Resolution references

- wasm-02 (`ext-proc-trailer-mutation-headermap-max-size`), Gateway data-plane
  panic audit 2026-11-08, reachable-panic, Medium; fixed 2026-11-12.
