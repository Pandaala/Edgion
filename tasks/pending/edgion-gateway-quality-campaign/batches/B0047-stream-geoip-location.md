# B0047 — Stream GeoIP Location

## Scope

- Baseline: `080e45a575617043c1596ce92c57296813786e5a`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Anchor file: `edgion-gateway/src/plugins/stream/geoip_location/plugin.rs`.
- Approved knowledge support: `skills/02-features/03-resources/14-edgion-stream-plugins.md`.
- Forbidden files: every other source/knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Exact commit allow-list: the anchor, approved feature page, and B0046/B0047 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; simple and not plan-first.
- The target module and operator-facing feature table call a rule-less L4 GeoIP configuration a valid no-op. Shared validation, Controller ingest, Gateway construction, operator guidance, and regression knowledge instead reject it because no L4 sink consumes resolved facts.
- Correct both statements to require at least one allow/deny rule group and retain fail-closed invalid-generation semantics.
- Preserve the sound runtime: L4 uses physical `DirectPeerIp`, mapped IPs are canonicalized, CIDR-only rules avoid MMDB lookup, geographic mode performs at most one lookup, reloads pin the old mmap, and existing `failOpen`/bounded observability behavior remains unchanged.
- Do not change runtime behavior, schema, tests, metrics, logging, provider ownership, or frozen integration files.

## Implementation

- Corrected the module contract to state that L4 requires at least one allow/deny rule group and that a rule-less generation is rejected fail-closed.
- Corrected the feature table's rule-less example and clarified bounded denial metrics, metric-only Stage-1 listener denial, and generic TCP/UDP protocol logs without GeoIP facts or sensitive reasons.
- Left runtime, schema, tests, logging, metrics, and provider ownership unchanged.

## Independent review

- The independent reviewer approved the validation correction but found one overbroad `never logged` claim in the changed feature table.
- The implementer corrected it within the existing allow-list; final re-review approved the complete two-file diff with no remaining findings.

## Verification

- Focused verification passed: 10 stream GeoIP, 2 invalid-generation, 14 shared configuration, 5 bridge, and 4 client-IP canonicalization tests.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only. Only the campaign's known lint and duplicate-dependency warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
