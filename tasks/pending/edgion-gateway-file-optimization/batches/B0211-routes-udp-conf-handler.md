# B0211 — UDP Configuration Handler

## Scope

- Pipeline state: committed locally.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/conf_handler_impl.rs`.
- Baseline: `ac3dfe7c`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0211 analyst.
- Investigate schema ownership, full/partial immutable generations and affected
  listener cleanup through actual UDP manager/unit and shared L4 callers.
- TCP/TLS precedents are inputs, not substitutes for UDP-specific evidence.
- Implementation follows the exact three-path scope below; existing integration and user-owned paths remain protected.

## Analysis Outcome

- Decision: change; simple. Target blob `18182707571467d7f894ed25524e645de0523859` at `ac3dfe7c`; no tests run during analysis.
- Full-set deep-clones UDPRoute and partial add/update uses unconditional `arc_map_into_owned` clones, then rewraps schemas in new Arcs. UDP unit compilation already accepts and stores the original schema Arc; shared partial helper supports this input without changes.
- Exact proposed scope: target, new `skills/04-review/routing/regression-guards/udp-route-schema-arc-sharing.md` and routing index. Keep the Result adapter; remove only unnecessary schema materialization.
- Preserve serialized rebuild, pre-mutation old-listener collection, remove/update/add ordering, affected union, changed/new versus sibling unit/selector identity and clear/sweep behavior. Existing UDP sessions pin upstreams and are not claimed to close immediately on route deletion.
- Retain all seven target tests; add input-Arc identity for full/add/update, multi-listener and sibling sharing, immutable old/new schema/plugin/backend generations and retained-manager clearing tests without global PGIS mutation. Run handler and UDP tests at promotion.
- No policy, logging, metrics, session or shared API change. Existing manager-cache documentation task owns stale UDP architecture claims; no new integration coverage is required for proving schema identity.

## Promotion and Implementation

- Original analyst revalidated at `86e2002c`: target, UDP directory, shared L4 helpers and owning knowledge unchanged since `ac3dfe7c`; TLS parser changes do not affect UDP.
- Implementer: `b0211_implementation`; authorized exact three-path schema ownership changes and local tests above. Any broader/complex deviation requires independent plan review first.

## Implementation and Verification

- Implementation complete in exactly three technical files. Handler blob `932f761c27afb846afa2904e2d9a0ed2da6dfcfe`; schema Arcs now pass through the existing full/partial compiler without deep materialization.
- All seven original tests retained and three inline tests added. Fresh handler 10/10 and UDP 34/34 passed, covering schema identity, shared/sibling unit and selector identity, immutable generations and retained-manager clearing.
- Implementer targeted format/diff and agent docs (1,265 files) passed. No integration/global PGIS/shared helper/logging changes.
- Independent reviewer: `b0211_final_review`; LGTM, no Must Fix or Should Fix at `86e2002c`, all three hashes unchanged. Personally passed UDP 34/34 including handler 10, format/diff/docs and checked real session consumption and retained-handle behavior. No UDP handler Clippy diagnostic or new warning.
- Coordinator routes 605/605, workspace format/check/Clippy, Gateway build, docs (1,265 files), SSA, metrics, Pingora source, Gateway tracing, English-only, diff and 3,120-file freeze passed. Clippy retains the same 983 Gateway lib-test warnings. Commit: `aec5caf5`; no push.
