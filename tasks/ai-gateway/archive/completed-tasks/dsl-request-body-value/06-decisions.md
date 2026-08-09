# Decisions

## Design review

An independent design-only review was completed after the initial code-level
design. The following findings were checked against the current implementation
and incorporated before coding.

## Design revision conclusion

### Adopted

- Base the cumulative scan budget on the resolved physical request-body cap,
  not the initial captured length, so a body enlarged by mutation can still be
  scanned according to the documented pass budget.
- Define every Body comparison: String/Body equality scans logical text;
  Body/Body equality is true because both opaque values identify the same
  request-scoped current body; Body ordering is a type error.
- Treat implementation milestones as internal checkpoints and publish the
  breaking `req.body` type change only with every promised migration API.
- Bound orphaned disk scans with a process-wide semaphore and cooperative
  cancellation checked between chunks. Reserve VM scan budget before launching
  a scan and refund only reported unused bytes on normal completion.
- Preserve last-duplicate-key JSON behavior without failing on an oversized
  earlier candidate. Track an oversized candidate as state, replace it if a
  later duplicate appears, and enforce the result limit only for the final
  candidate.
- Specify stock behavior and lock it with tests: statically referenced request
  bodies are gated even when a runtime branch would not evaluate them;
  unsupported capture fails closed, while a known bodyless request remains an
  empty Body.
- Define range decoding as raw byte slicing followed by isolated lossy UTF-8
  decoding, including when either boundary splits a valid code point.
- Define exact units and accounting for string limits, JSON paths, metadata
  operations, early-exit scans, failed scans, and rolled-back mutations.
- Add pre-implementation feasibility tests for incremental regex, selective
  JSON parsing, and stable Pingora replay snapshots.

### Not adopted

- Rejecting Body in boolean contexts was not adopted. `Value::is_truthy`
  already makes every DSL string truthy, including an empty string. An always
  truthy opaque Body preserves the existing Lua-style behavior and requires no
  hidden I/O.

### Reason

The adopted changes close correctness and resource-bound gaps without changing
the selected opaque-value architecture. The rejected item was based on an
incorrect assumption about current string truthiness.

## Main-agent additions

- A borrowed chunk visitor cannot cross `spawn_blocking` safely. A disk scan
  moves an owned immutable snapshot and an owned scanner into one blocking job;
  it returns only the operation result and accounting metadata.
- Every existing infallible `Value::into_string` call site must be audited.
  Body must produce a type error in headers, logs, interpolation, outbound call
  arguments, map keys, and other implicit coercion paths; it must never format
  as a placeholder string and continue.

## Post-implementation code review

An independent code review found two Must Fix issues; both were accepted and
fixed:

- A known-bodyless request exposed an empty Body but the new mutation methods
  did not install Pingora's bodyless replay owner. Body mutation session methods
  are now async and provision that owner before publishing a mutation. A
  regression test composes all mutation forms on an initially bodyless request
  and verifies the cached result and framing length.
- Body mutation arguments reused legacy scalar-to-string coercion, allowing
  values such as `nil` or `false` to become literal bytes. Append, prepend, and
  replace now require exact `Str` values; focused VM tests lock the TypeError
  contract.

The review's remaining findings were non-blocking: scan accounting is
source-chunk granular, conservative static stage detection may report mutation
before receiver type errors, and the bounded scan semaphores intentionally add
queueing without new high-cardinality observability.

## Final deep review

Three independent read-only reviews covered DSL core/VM semantics,
request-body storage and transport replay, and resources/docs/integration
contracts. The following low-risk findings were fixed:

- Preserve the legacy JSON number spelling for floats such as `1.0` and
  `-0.0`, both as selected scalars and inside selected arrays or objects.
- Retry positional file reads after a valid short `pread` instead of treating
  every short return as spill truncation.
- Mark the channel-and-thread selective JSON scanner as blocking so even small
  memory-backed bodies run it on the blocking pool rather than a Tokio worker.
- Advance the complete Pingora fork family to `c710f39c`, whose H2 bodyless
  replay seam requires the downstream stream to have reached END_STREAM.
- Distinguish pre-VM body-gate responses (413/408/500 with an empty body) from
  VM BodyAccess failures (fixed generic 500) in English, Chinese, and design
  documentation.
- Add stock-Pingora runtime tests plus real forced-spill cross-chunk regex,
  substring, JSON selection, and replay coverage.

The scan-accounting contract was finalized as conservative source-chunk
accounting. Scans fetch at most 64 KiB per source chunk and charge the complete
fetched chunk, including the final chunk where an early-exit scanner stops.
This matches the I/O and callback work already performed and avoids a
cross-scanner consumed-offset protocol whose extra complexity would not reduce
the underlying read cost. English, Chinese, design, and code-level contract
text now use the same definition.
