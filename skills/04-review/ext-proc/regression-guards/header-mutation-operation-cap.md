---
name: header-mutation-operation-cap
description: Use when reviewing how many ExtProc processor header-mutation entries Edgion accepts, HeaderMap MAX_SIZE panics on request, response, or ImmediateResponse headers, or parity with Envoy's ext_proc mutation limits.
knowledge_type: regression-guards
---

# ExtProc header mutations are capped at 256 set entries

## Conclusion and scope

One processor `HeaderMutation` applied to request headers, response headers, or an
`ImmediateResponse` local reply may carry at most `HEADER_MUTATION_SET_CAP` (256)
`set_headers` entries. `remove_headers` is not capped. `mutation::validate_mutation_size`
enforces this in the three existing validate-before-mutation functions in
`edgion-gateway/src/plugins/http/ext_proc/plugin.rs`:

- `validate_request_common_response` (request-headers and buffered request-body phases);
- `validate_response_headers_common` (response-headers phase);
- `immediate_action` (request- and response-phase ImmediateResponse, and the streamed
  request-body ImmediateResponse in `streamed.rs`), after the
  `disable_immediate_response` suppression early return.

A violation returns the detail `"too many set headers"`. It follows the existing
`bad verdict` path governed by `failureModeAllow`, and nothing in the verdict is applied
first: no header, body overwrite, or mode override.

Streamed request/response body verdicts already reject any header mutation. Response
trailers cap each operation list separately before cloning and validate the final result; see
[trailer mutation capacity](trailer-mutation-header-map-capacity.md).

## Rationale and evidence

- `http` 1.5.0 `HeaderMap::insert` calls `try_insert(..).expect("size overflows MAX_SIZE")`,
  and every insert, append, or entry call runs `try_reserve_one` first, even for an
  existing name. A write fails exactly when the map is in the displacement-danger
  (Yellow) state at raw capacity 32,768 with at least 6,554 distinct names, or when it
  already holds 24,576 names. Green maps hash with unkeyed FNV truncated to 15 bits, so
  about 130 crafted names can force the Yellow state. A map below 6,554 distinct names
  therefore never fails a write. The trailer page records the same bound.
- The Pingora fork's `insert_header_value` (`pingora-http/src/lib.rs`) still uses the
  panicking `insert`. Upstream commit `e21646b` hardened only the sibling
  `append_header_value` with `try_append`. Edgion reaches `insert_header` through
  `set_request_header_value`, `set_response_header_value`, and the queued
  `PendingHeaderOp` local-reply operations in `ctx.rs`.
- Before this cap, a processor could return about 25,000 distinct names (about 375 KB,
  under tonic's 4 MiB default decode limit) with `OVERWRITE_IF_EXISTS_OR_ADD`,
  `ADD_IF_ABSENT`, or as ImmediateResponse `Set` / `AddIfAbsent` operations. The request
  task then panicked and the downstream connection was dropped. `OVERWRITE_IF_EXISTS`
  cannot add names. `APPEND_IF_EXISTS_OR_ADD` itself goes through Pingora's
  non-panicking `try_append`, but appended names still grow the map, and the next
  panicking insert (Pingora's unconditional `Date` insert, Edgion's server header, or a
  later plugin write) would fail. That is why every `set_headers` entry counts,
  whatever its action.
- Envoy (`source/extensions/filters/http/ext_proc/mutation_utils.cc`) rejects a mutation
  whose `set_headers` or `remove_headers` count exceeds `max_headers_count` (default
  100) with `header_mutation_operation_count_exceeds_limit`. Edgion has no
  `max_headers_count` setting; 256 matches Pingora's HTTP/1 `MAX_HEADERS`
  (`pingora-core/src/protocols/http/v1/common.rs`) and the ExtProc `TRAILER_FIELD_CAP`.

## Alternatives and limits

- **Removals are not capped (deliberate difference from Envoy).** Removals cannot grow
  the map, so a removal cap only bounds CPU work. Envoy can cap removals safely because
  its connection manager also limits incoming downstream headers to `max_headers_count`.
  Edgion's HTTP/2 downstream admits about 1,900 headers (64 KiB header list), so a
  client could otherwise push a processor that strips every matching header (for
  example all `x-internal-*`) over a removal cap and, under `failureModeAllow: true`,
  get a forged header through. The independent review on 2026-11-12 found that case, and
  the user chose to drop the removal cap.
- **Trailer-specific removal limit.** Response trailer input already has a
  256-field limit. Both trailer operation lists are independently capped at 256
  before the first write. See the [trailer capacity guard](trailer-mutation-header-map-capacity.md).
  Request and response headers are written straight into Pingora's panicking map, with
  no cloned fallible candidate, so a bound is needed before application. Like Envoy's,
  this cap refuses some set lists whose final map would be valid.
- **Client-forced rejection of set-heavy processors.** A processor whose `set_headers`
  list grows with the client's header count (for example one that echoes every request
  header) can be pushed over 256 by an HTTP/2 client. Under `failureModeAllow: true`,
  the bad-verdict path (`fail_request_owner`, or `mark_terminal` in the response phase)
  then makes that ExtProc exchange terminal for the request: the
  verdict and every later ExtProc phase are skipped, and an oversized ImmediateResponse
  deny becomes a pass-through. Under fail-close it only fails the client's own request.
  Envoy instead answers a rejected mutation with an error response regardless of
  `failure_mode_allow` (per its `main` source in November 2026, not a pinned release).
  Edgion keeps its existing bad-verdict policy, so no new failure shape is added
  (AGENTS.md cross-cutting rule 10). Entries are counted before protected or invalid
  ones are filtered, as in Envoy, so an echoed pseudo-header also counts.
- **Envoy's post-mutation result check** (result count at most `max_headers_count`,
  size at most `max_headers_kb`) was not adopted: Edgion has no equivalent configured
  limit, and an HTTP/2 downstream may legitimately carry more than 256 headers.
- **Per mutation, not per request.** One ExtProc entry adds at most 512 names to the
  request map (request-headers plus buffered request-body phase) and 256 to the response
  map or a fresh local reply. With at most 256 HTTP/1 or about 1,900 HTTP/2 downstream
  request headers, or an HTTP/1 upstream response (at most 256), one entry stays below
  6,554. Roughly 9-13 hostile ExtProc entries on one route could reach it. That residual,
  other callers that can feed many names into `insert_header` (DSL `set_header` with
  raised `maxCallCount` / `maxLoopIterations`), and the unpatched fork root are tracked
  in `tasks/todo/header-map-insert-max-size-residuals.md`.
- **Fork root.** Patching `insert_header_value` to `try_insert` was not done; the fork's
  ownership policy (`../pingora/AGENTS.md`, "Ownership and fix policy") leaves
  upstream-inherited code to the normal upstream fix. The fork records the limitation in
  `../pingora/edgion-changes/review/upstream-limitations.md`.
- An HTTP/2 upstream response map that already arrives near the limit is a separate
  root: `tasks/block/core-01-upstream-h2-response-header-map-near-max-size.md`.

## Verification and re-evaluation

Verified on 2026-11-12. Earlier runs, before the removal cap was dropped: `cargo test -p
edgion-gateway --lib ext_proc` (168 passed) and the full `cargo test -p edgion-gateway
--lib` (6,862 passed, 19 ignored). Final run: `cargo test -p edgion-gateway --lib --
ext_proc plugins::http::wasm` (445 passed: 168 ExtProc, 277 Wasm). Tests:

- `mutation::tests::validate_mutation_size_caps_set_entries_but_not_removals`;
- `plugin::tests::request_header_mutation_over_operation_cap_follows_bad_verdict_policy`,
  `request_header_mutation_at_operation_cap_applies`,
  `response_header_mutation_over_operation_cap_follows_bad_verdict_policy`, and
  `response_header_mutation_at_operation_cap_applies` (the accepted cases carry 1,024
  removals);
- `plugin::tests::header_verdict_over_operation_cap_is_rejected_before_mode_override`;
- `plugin::tests::immediate_action_rejects_header_mutation_over_operation_cap_after_suppression`,
  `immediate_header_mutation_over_operation_cap_follows_bad_verdict_policy`, and
  `immediate_header_mutation_at_operation_cap_is_queued_for_local_reply`;
- `plugin::tests::buffered_request::request_header_operation_cap_applies_at_limit_and_rejects_one_more_per_policy`
  and `response_header_operation_cap_applies_at_limit_and_rejects_one_more_per_policy`
  (full `run_request` / `run_upstream_response` against a scripted processor);
- `streamed::tests::immediate_response_header_mutation_over_operation_cap_follows_failure_policy`.

With the three validator calls removed (first version of the fix), all 8 rejection
tests failed. No integration scenario was added: it needs a hostile processor, and the
bad-verdict path is pre-existing. `run_request_streamed` is covered only through the
shared validators.

Re-evaluate if any of these change:

- a new path applies a processor header mutation without passing one of the three
  validators;
- Edgion adds a configurable downstream header-count limit (then consider Envoy's
  removal cap and result check);
- the Pingora fork adopts a non-panicking `insert_header_value`;
- `http` changes the `insert` / `try_insert` contract.

## Resolution references

- Panic-audit wasm-01 (`header-insert-max-size-panic-ext-proc-and-wasm`, Gateway
  data-plane panic audit 2026-11-08), reachable-panic, Medium; fixed 2026-11-12 by this
  cap and the Wasm
  [per-phase header write cap](../../wasm/regression-guards/header-write-cap-per-phase.md).
