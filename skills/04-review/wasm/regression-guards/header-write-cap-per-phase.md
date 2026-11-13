---
name: header-write-cap-per-phase
description: Use when reviewing how many header writes a Wasm guest may queue per hook phase, proxy_set_header_map_pairs or proxy_send_local_response header counts, or HeaderMap MAX_SIZE panics when guest header operations are applied.
knowledge_type: regression-guards
---

# Wasm guest header writes are capped at 1,024 per hook phase

## Conclusion and scope

`MAX_HEADER_WRITES_PER_PHASE` (1,024, `edgion-gateway/src/plugins/http/wasm/limits.rs`)
bounds guest-controlled header writes in `host_abi.rs`:

- `proxy_replace_header_map_value` and `proxy_add_header_map_value` count one write each;
  `proxy_set_header_map_pairs` counts its guest pair count; removals do not count.
- The running total is `CallbackFrame::header_writes`. `HostState::checked_header_writes`
  checks it after the ABI retention preflight and before decoding, copying, reserving, or
  touching the snapshot or `pending_ops`. The new total is stored only next to
  `commit_abi_charge`, so a call that fails after the check consumes nothing. A call that
  would exceed the cap returns `STATUS_BAD_ARGUMENT` (2) and changes nothing.
- `proxy_send_local_response` replaces its header list rather than accumulating it, so it
  is bounded per call: more than 1,024 header pairs returns `STATUS_BAD_ARGUMENT` before
  the map is decoded, and any earlier accepted local response is kept.
- The counter lives in the frame, so `begin_phase` and `release_phase_payloads` reset it.
  Callout-response callbacks of the request-headers phase share that phase's frame
  (`resolve_callouts_on` in `callout.rs` never calls `begin_phase`), so their writes
  accumulate against the same cap. The guest has no ABI call that starts a new phase.

The callout request header path (`proxy_http_call`, finding wasm-03) is a separate
bound and is not covered here.

## Rationale and evidence

- `plugin.rs` `apply_ops` and `finish_local_response` apply queued writes through
  `set_request_header` / `set_response_header`, which reach the Pingora fork's
  `insert_header_value`. That still uses the panicking `http::HeaderMap::insert`. A write
  fails exactly when the map is in the displacement-danger (Yellow) state at raw
  capacity 32,768 with at least 6,554 distinct names, or already holds 24,576 names;
  about 130 crafted names can force the Yellow state (see the ExtProc
  [trailer capacity page](../../ext-proc/regression-guards/trailer-mutation-header-map-capacity.md)).
  A map below 6,554 distinct names never fails a write.
- The [ABI retention budget](../../cpu-memory/regression-guards/wasm-abi-retention-budget.md)
  (32 MiB / 200k ops per phase) bounds memory, not distinct names. It admitted a
  `proxy_set_header_map_pairs` or `proxy_send_local_response` map of about 30,000 small
  pairs, so a hostile or buggy guest could panic the request task.
- Envoy's Wasm host (`source/extensions/common/wasm/context.cc`) does not limit header
  counts in these calls. Envoy's own HeaderMap never panics, so there is no equivalent
  to copy.
- 1,024 leaves room for a guest that rewrites a whole header map with
  `proxy_set_header_map_pairs`, and matches the callout response header bound in
  `validate_response_headers` and the attached trailer admission. Only the
  request-headers, request-body, and response-headers phases apply writes (the
  standalone response-body filter's ops are dropped; response-body observers, trailers,
  and `finish` are read-only; tick ops are never applied). `finish_callback` applies
  queued ops and then local-response headers. Any of the three phases may write either
  map (the map id is not phase-gated, and the response-phase session adapter still
  writes the downstream request header). One plugin therefore adds at most 3,072 names to
  the request map and about 4,096 to the response map (response writes queued in both
  request phases, the response-headers phase, and one local response). With at most
  256 HTTP/1 or about 1,900 HTTP/2 downstream request headers, or an HTTP/1 upstream
  response (at most 256), a single plugin stays below 6,554.

## Alternatives and limits

- A 256 cap (the ExtProc value) was rejected: a guest that writes back a whole header
  map would fail on requests with a few hundred headers. Raising the cap to 2,048 was
  also rejected: the response map could then reach 8,192 names from one plugin.
- Status code: other per-phase ABI quotas return `STATUS_INTERNAL_FAILURE`;
  `STATUS_BAD_ARGUMENT` was chosen because the guest's argument (its write volume)
  exceeds a fixed contract rather than an exhausted shared resource. Go-SDK guests see a
  different error value than for the retention budget.
- Status precedence: the read-only check, guest-memory and malformed-map errors, the
  preflight transient-allocation check, and the preflight retention check still come
  before the cap. The growth-inclusive transient/retention checks and the map-id check
  now run after it, so a call that exceeds both returns `STATUS_BAD_ARGUMENT` instead of
  `STATUS_INTERNAL_FAILURE`. For `proxy_send_local_response` the cap now runs before the
  transient-allocation check. An invalid map id at the cap is unobservable (both return
  `STATUS_BAD_ARGUMENT`).
- Guest-visible effect: the Rust SDK's `hostcalls` (`set_map`, `set_map_value`,
  `add_map_value`, `send_http_response`) panic on any non-OK status, so a Rust guest
  traps and `failOpen` applies; under `failOpen: true` all of that guest's hooks for the
  request are skipped and a local-response deny with more than 1,024 headers lets the
  request continue. SDKs that return the status (Go, C++, AssemblyScript) see the error;
  if the guest ignores it, the write is silently skipped even with `failOpen: false`.
- Client-forced failure: a guest that writes back the whole request map (for example
  `set_http_request_headers(get_http_request_headers())`) fails once a client sends more
  than 1,024 headers, which HTTP/2 allows (about 1,900). Envoy-portable guests rarely
  meet this, because Envoy's connection manager limits incoming headers to 100 by
  default. Do not rely on a fail-open guest that rewrites whole maps to strip
  security-relevant headers.
- Counting is conservative. Repeated writes to one name each count, and pairs that
  `apply_ops` later skips as protected response headers count too.
- With the default retention budget, repeated single replace/add calls with distinct
  names usually hit `STATUS_INTERNAL_FAILURE` at about 800 calls, because `pending_ops`
  growth is charged per reservation. `proxy_set_header_map_pairs` and local responses
  were the realistic panic paths.
- The cap is per plugin. Two hostile Wasm entries on one route could push the request
  map (with a large HTTP/2 request) or the response map past 6,554. That residual and the unpatched fork root (`../pingora/AGENTS.md`,
  "Ownership and fix policy") are tracked in
  `tasks/todo/header-map-insert-max-size-residuals.md`; the fork records the limitation
  in `../pingora/edgion-changes/review/upstream-limitations.md`.
- An HTTP/2 upstream response map that already arrives near the limit is a separate
  root: `tasks/panic-issues/core-01-upstream-h2-response-header-map-near-max-size.md`.

## Verification and re-evaluation

Verified on 2026-11-12. Earlier runs, before the fifth test was added: `cargo test -p
edgion-gateway --lib plugins::http::wasm` (276 passed) and the full `cargo test -p
edgion-gateway --lib` (6,862 passed, 19 ignored). Final run: `cargo test -p
edgion-gateway --lib -- ext_proc plugins::http::wasm` (445 passed: 168 ExtProc, 277 Wasm,
including 53 in `wasm::host_abi`). New tests in
`tests/unit/plugins/http/wasm/host_abi_test.rs`:

- `header_map_pairs_cap_accepts_exact_limit_and_rejects_next_pair_atomically`;
- `header_value_writes_share_phase_cap_and_reset_with_new_phase` (raises the retention
  budget so only the new cap is exercised);
- `header_map_pairs_and_value_writes_count_against_one_cap`;
- `local_response_header_count_is_capped_per_call`;
- `rejected_header_writes_after_cap_check_do_not_consume_the_budget`.

No negative-control run (tests against the pre-fix source) was recorded. No test pins
that callout rounds share the phase counter, and no integration or real-SDK guest test
exercises the cap; the Rust SDK trap behaviour comes from reading proxy-wasm 0.2.5
`hostcalls.rs`.

Re-evaluate if any of these change:

- a new host call queues header writes, or guest writes are applied outside
  `apply_ops` / `finish_local_response`;
- more phases start applying header writes, or callout rounds start a new phase frame;
- the Pingora fork adopts a non-panicking `insert_header_value`;
- `http` changes the `insert` / `try_insert` contract.

## Resolution references

- Panic-audit wasm-01 (`header-insert-max-size-panic-ext-proc-and-wasm`, Gateway
  data-plane panic audit 2026-11-08), reachable-panic, Medium; fixed 2026-11-12 by this
  cap and the ExtProc
  [header mutation set cap](../../ext-proc/regression-guards/header-mutation-operation-cap.md).
