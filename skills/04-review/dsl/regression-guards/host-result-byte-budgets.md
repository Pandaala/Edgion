---
name: dsl-host-result-byte-budgets
description: Use when reviewing Redis HGETALL conversion or HTTP-call response-header construction before DSL VM admission.
knowledge_type: regression-guards
---

# DSL external-result producers enforce cumulative byte budgets

## Conclusion and scope

Gateway DSL `redis.hgetall` and `http.call` response headers enforce a fixed
`MAX_DSL_HOST_RESULT_BYTES = 1 MiB` per-result payload cap. Count field/name
bytes plus value bytes with checked arithmetic before Edgion copies the result.
The limit excludes container/allocator overhead and fixed result envelopes;
it is independent of VM admission, `maxMapSize`, and `maxStringLen`.

Redis checks the original, untruncated fields and values. Count or byte overflow
returns the existing `ok: false`, `value: nil`, `result_too_large` failure (or
the generic `e` under a small string budget). Admitted results retain UTF-8-safe
per-string truncation. HTTP measures the current retained header Map, refunds
the old value on replacement, and counts each name once. Overflow returns the
complete status-zero failure Map before reading the body.

## Rationale and evidence

The originating resource-safety task is
`bound-dsl-host-result-aggregate-bytes`. Before this correction,
`RedisLinkClient::hgetall` converted the complete result into
`HashMap<String, String>` before the DSL checked its count. HTTP response
completion checked only the retained header count and each string's length.
The VM's later `VmState::push` shape/work inspection could not prevent these
earlier allocations.

The Gateway provider requests `redis::Value` and checks original entry counts
and byte lengths before converting to owned strings. RESP2 arrays must have an
even length; maps use original entry count. Invalid UTF-8 fails conversion only
after limits have been checked. `checked_hash_bytes` rejects arithmetic overflow.
The client already aggregates the complete response before this check; this is
not a wire-frame or total-memory limit. Nested values and error frames never
become user payload. Revalidate when changing redis-rs decoding or conversion.

## Alternatives and limits

The execution entry requests raw `redis::Value` before owned-string conversion.
This preserves command ACL, single-node key routing, and the total deadline.
It introduces no second decoder and does not replace HGETALL with HSCAN/Lua.

The guard cannot cap network buffering or the complete decoded RESP frame,
including the decoder's native RESP3 Map. No total-process-memory or
OOM-prevention claim is made. HSCAN would change a single HGETALL read into
multiple observations and is outside this correction.

HTTP already has the canonical client's decoded HTTP/2 header-list limit:
16 KiB by default, configurable up to 64 KiB. The DSL cap is an independent
producer guard, not evidence of an unbounded HTTP/2 network path. The HTTP body
still uses the canonical capped streaming helper. Neither path adds a cache,
tracing, metrics, response BodyStorage, or a new failure-policy shape.

HTTP preserves skipping non-UTF-8 values, per-string checks, lowercase names,
duplicate replacement only while traversal is open, and ignoring the tail
after the retained Map is full. Redis count checks precede byte inspection;
odd-length Arrays, Null, unexpected frame types, and malformed admitted entries
remain dependency errors, with detail restricted
to the existing sensitive PluginLog path. A raw oversized result fails even
when truncation could have reduced its eventual DSL payload.

## Verification and re-evaluation

Focused regressions cover raw Redis UTF-8 byte lengths, exact/over-cap payloads,
RESP2 Array and RESP3 Map count boundaries, many large entries, rejection
before additional Map/string conversion, empty replies, command-error frames,
arithmetic overflow, malformed values, fixed failure classes and UTF-8
truncation. Synthetic HTTP response tests cover exact/over-cap headers,
cumulative overflow, duplicate replacement in both size directions, and no
body polling on header overflow. An oversized RESP2 Array containing nested
error frames returns the limit result even though direct redis-rs frame conversion
fails, locking the count check before additional result conversion. Existing
HTTP prefix and body tests remain.

Verified on 2026-10-05:

- `cargo test -p edgion-gateway --lib link_sys::providers::redis::ops::tests -- --nocapture`
  passed 12/12 provider tests, including raw RESP2/RESP3 replies, early count
  rejection before redis-rs conversion, exact 1 MiB, and checked-overflow cases.
- `cargo test -p edgion-gateway --lib plugins::http::dsl:: -- --nocapture`
  passed 467/467 DSL tests, including the new response-header and Redis result
  regressions and existing prefix, body, sandbox, and failure-policy coverage.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed.
- Agent-doc validation, SSA-force, metrics inventory, Pingora source policy,
  Gateway API transform contract, unit-test layout, Gateway no-tracing, and
  `git diff --check` passed.
- An independent read-only Astra reviewer returned LGTM for code, callers, tests,
  bilingual documentation, knowledge classification, and dependency limits.
- No full `cargo test --all` or live integration run is claimed. This change
  does not alter configuration reload or client-generation resolution;
  existing integration fixtures remain unchanged.

Re-evaluate if redis-rs introduces bounded reply decoding or a bounded HGETALL API,
custom-command routing/flush or HGETALL conversion semantics change,
the cap or result envelopes change, or response-header
collection no longer retains the current raw-prefix contract.

## Resolution references

- `bound-dsl-host-result-aggregate-bytes` — confirmed producer-side resource
  defect corrected at the earliest public raw-frame inspection boundary;
  dependency-side aggregation remains explicitly outside that guarantee.
