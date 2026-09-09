# B0211 — UDP Configuration Handler

## Scope

- Pipeline state: analysis-ready; third queued candidate, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/conf_handler_impl.rs`.
- Baseline: `ac3dfe7c`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0211 analyst.
- Investigate schema ownership, full/partial immutable generations and affected
  listener cleanup through actual UDP manager/unit and shared L4 callers.
- TCP/TLS precedents are inputs, not substitutes for UDP-specific evidence.
- No implementation authorized; existing integration and user-owned paths remain protected.

## Analysis Outcome

- Decision: change; simple. Target blob `18182707571467d7f894ed25524e645de0523859` at `ac3dfe7c`; no tests run during analysis.
- Full-set deep-clones UDPRoute and partial add/update uses unconditional `arc_map_into_owned` clones, then rewraps schemas in new Arcs. UDP unit compilation already accepts and stores the original schema Arc; shared partial helper supports this input without changes.
- Exact proposed scope: target, new `skills/04-review/routing/regression-guards/udp-route-schema-arc-sharing.md` and routing index. Keep the Result adapter; remove only unnecessary schema materialization.
- Preserve serialized rebuild, pre-mutation old-listener collection, remove/update/add ordering, affected union, changed/new versus sibling unit/selector identity and clear/sweep behavior. Existing UDP sessions pin upstreams and are not claimed to close immediately on route deletion.
- Retain all seven target tests; add input-Arc identity for full/add/update, multi-listener and sibling sharing, immutable old/new schema/plugin/backend generations and retained-manager clearing tests without global PGIS mutation. Run handler and UDP tests at promotion.
- No policy, logging, metrics, session or shared API change. Existing manager-cache documentation task owns stale UDP architecture claims; no new integration coverage is required for proving schema identity.
