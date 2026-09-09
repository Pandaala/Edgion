---
name: tcp-route-schema-arc-sharing
description: Use when reviewing TCP route initialization and immutable schema ownership during full and incremental configuration updates.
knowledge_type: regression-guards
---

# TCP route schema Arc sharing

## Conclusion and scope

`GlobalTcpRouteManagers` compiles incoming `Arc<TCPRoute>` values directly into
`TcpRouteUnit` companions. Full replacement clones only the Arc handle;
incremental add/update moves the incoming Arc through `apply_l4_partial_update`.
The schema remains immutable and is shared with the configuration cache.

## Rationale and evidence

The former handler deep-cloned the full schema in `full_set` and used
`arc_map_into_owned` for deltas before allocating a replacement schema Arc.
`TcpRouteUnit::from_route` already consumes an Arc and only reads its schema,
so those copies served no mutation requirement. This correction reduces cold
configuration-path copying under the rebuild lock; it makes no measured
request-latency claim. It follows [Rust clone guidance](../../../03-coding/02-rust-coding-rules.md)
and preserves the [ConfHandler incremental contract](../../../01-architecture/02-gateway/11-conf-handler-guidelines.md).

## Alternatives and limits

Compiled units and backend selectors are still built for each changed route.
Listener table rebuilds reuse cached units, keeping unchanged sibling selectors
and their WRR state. Retained old units and snapshots keep their prior schema
generation. The Result adapter, shared delta helper, rebuild lock, deterministic
selection, and per-listener atomic publication remain unchanged; no cross-store
atomicity is claimed. No TLS/UDP ownership change is implied.

## Verification and re-evaluation

Inline tests in `edgion-gateway/src/routes/tcp/conf_handler_impl.rs` cover incoming
schema pointer identity for full-set/add/update, old-generation preservation,
listener migration clearing the retained old manager, new-listener selection,
and unchanged sibling unit/selector identity. Existing misclassified-add and
route-manager clear-pass tests remain. Arc identity is an in-process invariant,
not a cross-process integration-test claim.

Verified with `cargo test --locked -q -p edgion-gateway --lib routes::tcp`:
18 passed, including all existing TCP tests. Targeted rustfmt check, `git diff
--check`, and `python3 cicd/checks/validate_agent_docs.py` also passed.

Re-evaluate if TCP schema compilation gains a genuine mutation requirement;
do not reintroduce unconditional full-schema cloning merely to fit a helper.

## Resolution references

- B0198: TCP configuration handler schema-copy reduction.
