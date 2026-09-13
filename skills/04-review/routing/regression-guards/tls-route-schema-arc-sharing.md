---
name: tls-route-schema-arc-sharing
description: Use when reviewing TLS route initialization and immutable schema ownership during full and incremental configuration updates.
knowledge_type: regression-guards
---

# TLS route schema Arc sharing

## Conclusion and scope

`GlobalTlsRouteManagers` compiles incoming `Arc<TLSRoute>` values directly into
`TlsRouteUnit` companions. Full replacement clones only the Arc handle; incremental
add/update moves the incoming Arc through `apply_l4_partial_update`. Compilation
reads the immutable schema and builds separate gateway runtime state.

## Rationale and evidence

The former handler deep-cloned the schema in `full_set` and used
`arc_map_into_owned` for deltas before allocating another schema Arc.
`TlsRouteUnit::from_route` already accepts an Arc and only borrows its fields;
both terminate and passthrough proxies likewise borrow the schema. These copies
served no mutation requirement. The correction removes cold configuration-path
copying under the rebuild lock, not a measured request-latency cost. It follows
[Rust clone guidance](../../../03-coding/02-rust-coding-rules.md) and the
[ConfHandler contract](../../../01-architecture/02-gateway/11-conf-handler-guidelines.md).

## Preserved boundaries

Compiled units and backend selectors remain per changed route, shared across its
SNI buckets. Unchanged sibling units retain selector identity and WRR state.
The Result adapter, shared delta/rebuild helpers, locks, controller-resolved
listener/hostname fields, first-rule selection, denied backend weights, and
per-listener atomic publication are unchanged. No cross-listener atomicity or
hostname-projection correction was claimed by that change. The later
[attachment-scoping correction](tls-hostnames-per-attachment.md) preserves schema
sharing while replacing the controller-resolved hostname representation.

## Verification and re-evaluation

Inline tests in `edgion-gateway/src/routes/tls/conf_handler_impl.rs` assert input
schema pointer identity for full-set/add/update, multi-SNI unit/selector sharing,
unchanged sibling identity, listener migration clearing a retained old manager,
new-generation hostname/backend selection, and retained old snapshot/schema/runtime
behavior. All seven pre-existing handler tests remain. Arc identity is an
in-process invariant, not a cross-process integration claim.

Verified with `cargo test --locked -q -p edgion-gateway --lib routes::tls`:
43 passed, including all eight handler tests. Targeted rustfmt and `git diff
--check` passed.

Re-evaluate if compilation gains a genuine schema mutation requirement; do not
reintroduce unconditional schema copying merely to fit a helper.

## Resolution references

- B0203: TLS configuration handler schema-copy reduction.
