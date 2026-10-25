# Evaluate scan reuse in DSL split

Status: deferred for user decision. No runtime change.
Source baseline: bb98e6479.

## Evidence and current boundaries

`BuiltinId::Split` in Gateway `plugins/http/dsl/lang/builtins.rs` first converts
the delimiter and input through RuntimeString. For an empty delimiter it counts
Unicode scalars plus two boundary elements; otherwise it counts non-overlapping
delimiter matches plus one. It returns `ListTooLong` with the full count and
configured limit before allocating a result vector or element Arcs.

After admission, it reserves the already-known count and traverses `str::split`
to build independent Arc strings. Successful calls therefore scan for counting
and again for splitting. Rejected calls perform counting without result
construction. Exact result capacity was already optimized in an earlier batch;
this task does not rediscover vector growth or claim a measured bottleneck.

## Decision requested

Retain the simple two-pass implementation, or profile a bounded design that
reuses split boundaries. Saving source offsets or borrowed spans can avoid a
second search but requires transient metadata, construction admission and
ownership rules. A one-pass result builder allocates before the current result
admission boundary; stopping at limit plus one changes the exact error count.
Neither is an equivalent cleanup. No prototype or replacement is selected.

## Acceptance before implementation

- Use IMPL with a frozen allow-list and preserve delimiter/input pop and
  coercion order, error policy, call limits and exact `ListTooLong` values.
  Preserve result rejection before Vec/Arc construction; any proposed metadata
  allocation has its own independently reviewed bound and failure policy.
- Preserve empty input, empty delimiter's leading/trailing elements, UTF-8
  scalar boundaries, repeated/trailing delimiters, multi-byte delimiters,
  non-overlapping matches and exact/over-list-limit results. Keep independent
  Arc element ownership and item order; no source-retaining string-view type.
- If a candidate stops retaining metadata at the limit, it must still establish
  the complete count before returning the existing error. Input/coercion work
  and temporary storage must not be hidden by output-only accounting.
- Measure scan work, temporary metadata, allocations and retained capacity for
  short/large inputs, sparse/dense/no matches, empty delimiters and rejected
  results. A second pass over a contiguous string may be cheaper than retaining
  spans; no throughput or memory win follows from counting source traversals.
- Preserve the current prepared script/VM and canonical Value representation.
  No global/request cache, new builtin, regex substitution, tracing or metric
  is authorized. DSL-01 argument ownership remains a separate decision.

The existing range/string and sandbox tests cover Unicode empty-delimiter counts,
exact/full-count over-limit errors, iteration and result access. They are
regression evidence for current behavior, not acceptance of new scratch storage.
See `skills/01-architecture/02-gateway/dsl-engine-security-model.md` for
pre-allocation list admission and the source/tests for the current exact count.
