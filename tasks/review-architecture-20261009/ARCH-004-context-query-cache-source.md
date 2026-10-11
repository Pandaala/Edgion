---
id: ARCH-004
status: candidate
kind: architecture-debt
priority: P3
primary_partition: GW-CONTEXT
analyst: /root/analyze_session
independent_verification: confirmed
---

# Make the context own the frozen source of its query cache

## Current ownership and evidence

`ctx.rs:3412` exposes `cached_query(&self, raw_query: Option<&str>)`, although
EdgionHttpContext owns both frozen `request_info.entry_raw_query` and the permanent
query OnceLock. Documentation instructs callers to pass that frozen source and
states subsequent arguments are ignored.

All three production readers repeat that instruction: query route matching in
`routes/http/match_unit.rs:539`, plugin key lookup in
`plugins/runtime/session_adapter.rs::get_query_param`, and access-log projection
in `observe/access_log/log_ext.rs:262`. Original URI architecture requires these
single-key readers to observe the frozen entry query independently of rewrites.
Whole-query access intentionally reads the live URI through a different operation.

## Change scenario and maintenance cost

Adding a condition, template source or log reader requires knowing that this
apparently general cache adopts only its first caller's supplied input. Passing
a post-rewrite live query or premature None would permanently choose that source
for all later readers. Existing cache tests demonstrate first-input permanence,
including None followed by nonempty input.

The observed cost is an owner-known source invariant delegated to caller discipline
and repeated by three consumers. All current production callers pass the intended
source; no current bypass, incident or measured runtime regression is claimed.

## Legitimate complexity and bounded recommendation

Keep shared lazy request-local decoding and init-once borrowed reads, including
the allocation-free empty singleton. Change the context accessor to take no
source argument and obtain its own entry_raw_query when initializing. Update the
three callers and focused tests, configuring entry snapshots in test contexts.

This gives the existing owner responsibility for its existing invariant; it adds
no cache, wrapper, decoder, allocation or lock and requires no broad RequestInfo
privacy rewrite. The main risk is accidentally switching to live query semantics
or altering initialization timing. Separate whole-query behavior remains unchanged.

## Invariants and future verification

Preserve frozen-key behavior through URI/query credential rewrites, canonical
percent/plus decoding, one initialization and borrowed maps, zero empty-view
allocations, routing/plugin/log agreement, live whole-query access, execution
order and canonical access-log redaction. Adapt existing cache tests and cover
context-owned source selection despite live URI changes. This source-only review
ran no tests/builds and creates no integration fixture requirement by itself.

## Prior records and independent disposition

Original URI architecture and normalization contract, context continuity, trace
identity, scoped publication, body admission, terminal/observability decisions
and focused context tests were read. Pending/todo/block task and review searches
found no competing cached_query/entry_raw_query issue. This root is separate from
feature capability growth and generic body-contract ownership. Independent
verification remains pending, particularly the value of this narrow API correction.
