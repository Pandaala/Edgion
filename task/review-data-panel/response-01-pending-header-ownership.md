# Decide pending response-header execution ownership

Status: deferred for user decision. Scope: response-header queue execution.

## Evidence

`EdgionHttpContext::apply_pending_response_header_ops` in
`edgion-gateway/src/ctx.rs` clones each `PendingHeaderOp` before calling its
consuming `apply`. Every variant owns a String name; write variants also own a
typed HeaderValue. Protection checks, conditional presence checks and Remove
execute only after that clone. Remove, protected operations and false
conditions therefore pay for copies they do not need for final insertion.

The queue itself must survive the normal upstream-response application:
`build_response_local_reply` can later replay it onto a fresh response through
`drain_pending_response_header_ops`. Drain already moves operations. Successful
forwarded writes and retained queue state need independent ownership; eliminating
all copies or consuming the queue on first use is not a valid optimization.

## Decision

Choose whether the reduced allocation warrants a shared borrowed/owned execution
path. It should decide protection and conditional no-ops before materializing
owned insertion inputs, while retaining the existing move-based drain. Avoid two
independent implementations of conditional/protection semantics, and avoid making
the destructive drain clone values merely to share a borrowed API.

Prevalidating names into HeaderName is a separate possible extension, not an
approved part of that optimization. It changes producer interfaces and may alter
invalid-name handling, validation timing or Pingora's original-case map. Audit
every producer and exact serialization behavior before choosing that design.

## Required acceptance

- Apply each condition against the current header after preceding operations;
  preserve ordered Append and repeated Set-Cookie values.
- Retain best-effort invalid-name handling and canonical protected-header checks.
- Preserve opaque HeaderValue bytes and sensitivity metadata; add no diagnostic
  containing names or values.
- Keep the queue available for local-reply replay and consume it only at the
  existing drain boundary. Do not carry unrelated upstream sticky cookies over.
- Compare normal replay and destructive drain for all five variants, present and
  absent headers, invalid/protected names, opaque values and local replacement.
- Measure skipped/Remove allocation savings and successful-write/drain costs;
  do not infer throughput from clone counts.

Original constraints: `skills/04-review/ext-proc/regression-guards/opaque-header-fidelity.md`,
`skills/04-review/plugin-runtime/regression-guards/response-header-mutation-policy.md`
and `skills/04-review/routing/intentional-design/local-reply-drops-sticky-set-cookie.md`.
This task records source inspection only; it implements no ownership change.
