# Decide endpoint probe response text ownership

Status: deferred for user decision. Scope: AllEndpointStatus bounded probe body
conversion after its existing streaming read, not general proxy response storage.

## Evidence

In `edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs`,
read_body_limited starts a Vec with capacity max_size.min(8192), appends bounded
stream chunks and returns the buffer plus the truncation flag. Capacity may grow
for larger bodies. fan_out_requests records the raw byte length, then calls
String::from_utf8_lossy(&body_bytes).into_owned(). Nonempty valid UTF-8 is copied
into a text owner; invalid UTF-8 already produces an owned replacement String
which into_owned transfers. Empty input does not establish an allocation cost.

A direct String::from_utf8(body_bytes) transfer could remove the valid-text copy.
It also retains the read buffer's capacity in each completed result. For a tiny
response, that can retain the initial 8 KiB reservation until collected results
are assembled and serialized, including while slower probes are still running.
The current text copy allows the larger read buffer to be released earlier.
This is a capacity/lifetime tradeoff, not evidence of an unconditional memory win.

## Decision

Compare keeping the copy, transferring the buffer, and a measured compaction
policy. Account for allocations and copies, retained capacity, peak concurrent
memory, invalid UTF-8 fallback work, and result lifetime while slow peers finish.
A shrink operation or threshold adds work and policy; it is not automatically
cheaper. Include empty/tiny/large responses, invalid UTF-8, truncated multibyte
tails, read errors, cancellation and mixed completion times.

The [address ownership task](endpoint-01-result-address-ownership.md) concerns
address copies and their timing boundary. The
[endpoint projection task](endpoint-02-bounded-endpoint-projection.md) concerns
selection work before fan-out. Neither is implemented or closed by this proposal.

## Required acceptance

- Preserve lossy replacement text exactly and keep response_body_bytes and
  aggregate totals based on raw received bytes, not replacement-text length.
- Keep the existing streaming byte cap and explicit body_truncated diagnostics,
  including the current extra-chunk boundary behavior and partial-body return
  after a stream error. This is no permission for silent truncation or a general
  plugin aggregation path.
- Preserve per-endpoint and wall deadlines, permit/cancellation cleanup, ordinal
  identity, partial completion and serialized null/error/header conventions.
  Identify any changed work inside timed intervals in the reviewed design.
- Keep client isolation, policy generation checks, TLS identity, endpoint pinning
  and SSRF checks. Add no shared cache, response BodyStorage, disk spool or new
  request-level logging.
- Verify pure conversion equivalence and retained capacity alongside deterministic
  fan-out tests. Do not claim network deadline or throughput equivalence from a
  helper-only test. Independently review the implementation scope and result.

This documentation-only task adds no runtime code, tests or measured performance
claims. Existing probe tests are baseline evidence, not measurements of this
ownership alternative.
