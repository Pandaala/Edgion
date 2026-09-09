---
name: udp-route-schema-arc-sharing
description: Use when reviewing UDP route initialization and immutable schema ownership during full and incremental configuration updates.
knowledge_type: regression-guards
---

# UDP route schema Arc sharing

## Conclusion and scope

`GlobalUdpRouteManagers` compiles incoming `Arc<UDPRoute>` values directly into
`UdpRouteUnit` companions. Full replacement clones only the Arc handle; incremental
add/update moves the incoming Arc through `apply_l4_partial_update`. Compilation
reads the immutable schema and builds separate gateway runtime state.

## Rationale and evidence

The former handler deep-cloned the schema during full replacement and used
`arc_map_into_owned` for deltas before allocating another schema Arc.
`UdpRouteUnit::from_route` already accepts an Arc and only reads its fields, so
these copies served no mutation requirement. The correction removes cold
configuration-path schema copying under the rebuild lock, not a measured latency
or RSS cost. It follows [Rust clone guidance](../../../03-coding/02-rust-coding-rules.md)
and the [ConfHandler contract](../../../01-architecture/02-gateway/11-conf-handler-guidelines.md).

## Preserved boundaries

Changed routes still receive new compiled units and first-rule backend selectors.
Listener snapshots share cached units; unchanged siblings retain selector identity
and WRR state. The Result adapter, shared delta/rebuild helpers, rebuild lock,
old-listener collection, remove/update/add order, affected-listener union, retained
manager clearing, and per-listener atomic publication remain unchanged. No
cross-listener atomicity is claimed.

Denied backend weights and the post-select fail-close gate are unchanged.
Stream-plugin keys still use the shared resolver and dynamic fail-close lookup;
sharing a schema does not snapshot plugin-store contents. Existing UDP sessions
keep their pinned upstream until session removal or idle expiry. Clearing route
tables affects new selection, not termination of already-pinned sessions.

## Verification and re-evaluation

Inline tests in `edgion-gateway/src/routes/udp/conf_handler_impl.rs` assert incoming
schema pointer identity for full-set/add/update, multi-listener unit/selector
sharing, unchanged sibling identity, old/new schema/backend/plugin-key generations,
and migration/delete/empty-full-set clearing through retained manager handles.
All seven pre-existing handler tests remain. Tests use in-process managers and do
not mutate the global listener store or exercise network services. Arc identity
and retained snapshots are in-process evidence, not a cross-process hot-reload
or live-session revocation claim.

Fresh verification: `cargo test --locked -q -p edgion-gateway --lib routes::udp`
passed all 34 tests; the handler filter passed all 10 tests. Targeted rustfmt,
`git diff --check`, and `python3 cicd/checks/validate_agent_docs.py` passed.

Re-evaluate if UDP compilation gains a genuine schema mutation requirement;
do not reintroduce unconditional schema copying merely to fit a helper.

## Resolution references

- B0211: UDP configuration handler schema-copy reduction.
