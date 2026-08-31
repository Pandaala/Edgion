# B0059 — GeoIP Updater Hardening

## Scope
- Baseline: `be501912c071eade8078edec0978525035eb0174`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned source files: `edgion-gateway/src/geoip/{config.rs,info.rs,mod.rs,provider.rs,updater.rs}`
- Exact implementation files: `edgion-gateway/src/geoip/config.rs`; `edgion-gateway/src/geoip/updater.rs`
- Exact documentation support: `skills/02-features/02-config/01-gateway-config.md`; `docs/en/ops-guide/gateway/geoip.md`; `docs/zh-CN/ops-guide/gateway/geoip.md`
- Allowed tests: colocated unit tests in the two implementation files and existing `edgion-gateway/tests/geoip_fixture_readback.rs`
- Forbidden files: every existing `edgion-tests/**` path, other Gateway modules, shared crates, schemas, metrics definitions, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the two implementation files; three documentation files; campaign ledger/changelog/main record; B0058 and B0059 batch records; one reviewer-approved D0025 record and canonical task; `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0055_runtime_analysis`, revalidated against `be501912`
- Baseline: all five targets are unchanged since the earlier LinkSys baseline; `cargo test -p edgion-gateway --lib geoip::` passed 46 tests.
- Security opportunity: updater Authorization is stored as a plain `String` and inserted without `HeaderValue::set_sensitive(true)`, so Secret-derived credentials lack HTTP sensitivity/never-indexed provenance.
- Availability opportunity: a valid sidecar can retain validators after the database or provider disappears; accepting a conditional 304 then sleeps the normal update interval and can preserve fail-open admission or fail-close outage indefinitely.
- Configuration opportunity: when updates are enabled, zero connect, download, stall, or steady retry durations create unusable or rapid-retry behavior; disabled updater fields must remain inert.
- Documentation opportunity: state positive duration constraints, validator trust requirements, and correct the stale memory-bound sentence to the canonical `max_db_bytes + 2 × current database size` contract.
- Expected no-change files: `info.rs` retains its request-owned projection and IPv4-mapped normalization; `mod.rs` retains coherent startup-only initialization, ArcSwap publication, same-directory guard, and bounded observability.
- Deferred candidate: `provider.rs` collapses MMDB decode errors and caught panics into ordinary `NotFound`, bypassing GeoIP consumer failure policy; this requires a cross-consumer semantic design rather than a local enum edit.

## Proposed implementation contract
- Parse the configured Authorization value once into `HeaderValue`, mark it sensitive immediately, store the typed value, and clone it into requests. Invalid values name only the environment variable and never echo its contents.
- Load persisted validators only when the durable database exists and the provider is active. Treat 304 without both conditions as a bounded fetch failure, clear validators, and enter cold backoff so the next request is unconditional.
- Validate every updater duration or interval as strictly positive only when `update_url` is nonempty; preserve disabled-field inertness and all existing defaults.
- Preserve the canonical managed outbound client, egress/redirect/TLS/DNS policy, size/stall bounds, atomic swap, metric names/labels, reason vocabulary, and logging redaction.

## Integration-test impact
- Existing `edgion-tests/**` remains frozen. Existing phase-4 coverage already exercises hot swap and sidecar creation and must not change.
- Source-local state/header/config tests and the existing fixture readback test are sufficient for this batch; no new integration-test issue is needed.

## Plan and implementation
- Implementer: `b0056_implementation`
- Complexity: complex enough for plan-first review because validator trust, provider availability, 304 handling, and retry state must remain coherent
- Plan: validate all five updater time/interval fields and the byte cap only when enabled; parse Authorization once into a sensitive typed header; make validator eligibility require both a durable regular database and active provider at load, request, and 304 response time; clear and best-effort persist validators before request construction whenever eligibility is lost; record whether a validator was actually sent; clear validators and force existing cold backoff after an untrusted 304 so the next GET is unconditional; use pure seams for new invalid-304 tests and isolated providers for every updater test; retain legal 304, swap, steady retry, metric/log, managed outbound, egress, TLS, DNS, streaming, and size contracts; document sensitive metadata without implying encryption or blanket HTTP/1 redaction.
- Support-file decision: no implementation support file is required; the existing fixture readback test is verification-only
- Actual change: updater configuration rejects enabled zero durations; Authorization is parsed once into sensitive typed metadata; validator restoration, request construction, and 304 acceptance require a regular durable database plus active provider; ineligible or untrusted state is cleared and persisted before cold unconditional recovery; all updater tests use isolated providers; the three authorized documents now state exact constraints, recovery, disk bound, and sensitive-metadata limits.

## Independent review
- Plan reviewer: `b0056_final_review`
- Plan verdict: Approved after five required revisions
- Plan correction summary: request-time ineligibility now clears and persists empty validators before building the request; a valid empty-sidecar regression proves mtime is not reseeded; new invalid-304 coverage uses pure seams rather than another unit-test server; every updater test receives an isolated provider; and documentation narrowly describes clone-preserved sensitive metadata, cooperative Debug masking, and HTTP/2 never-indexed behavior.
- Final code reviewer: `b0056_final_review`
- Final verdict: Approved after one correction re-review, with no residual Must Fix or Should Fix
- Must Fix resolution: `UpdateState::load` now owns one canonical eligibility observation and immediately persists empty state when ineligible, closing the startup double-observation window; missing-database and inactive-provider regressions prove stale validators cannot resurrect after eligibility returns; module and bilingual overview text now distinguishes legal and untrusted 304 behavior.
- Deferred adjudication: D0025 records the real, nonduplicate cross-consumer lookup-failure policy gap; it remains outside B0059 and needs no integration-test issue yet

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| GeoIP baseline tests | passed | 46 tests |
| Plan review | passed after revision | five exact state, test, and documentation corrections incorporated before implementation |
| Implementation and focused tests | passed | 7 config, 31 updater, 55 aggregate GeoIP, and 2 fixture-readback tests |
| Gateway all-target compile | passed | implementation verification |
| Agent documentation | passed | 1,114 files checked during implementation verification |
| Independent code review | passed after correction | updater 31/31 and formatting independently rerun after the startup-race fix |
| Final focused tests | passed | 55 aggregate GeoIP and 2 fixture-readback tests after formatting |
| Full formatting | passed | `cargo fmt --all` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace Clippy | passed | exit 0 with existing repository warnings |
| Final agent documentation | passed | 1,114 files, 36 skill entries, and 711 source-path scopes checked |
| SSA force guard | passed | 1,432 files checked; one existing allowlisted site |
| Metrics inventory | passed | 103 metrics in sync |
| Pingora source policy | passed | 16 fork packages share `edgion_v3` and one locked revision |
| Gateway no-tracing guard | passed | data-plane boundary and cold-function registry clean |
| Final frozen integration paths | passed | user-authorized concurrent baseline unchanged by B0059 |

## Handoff
- Ledger/changelog updated: yes; two changed, two examined-no-change, and one deferred to D0025
- Commit: `564e77ba`
