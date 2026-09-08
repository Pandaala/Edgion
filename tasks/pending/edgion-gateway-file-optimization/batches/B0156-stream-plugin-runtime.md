# B0156 — Stream Plugin Runtime

## Scope

- Analysis HEAD: `d9efd857`; promotion HEAD after external updates: `1c55ab7b74a4a56781d87ad2b30da10ccc876258`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs`
- Baseline blob: `75ed77da9498ae5af6d50a357a3e2439ef000b91`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0142_test_helpers_analysis` audited all four plugin variants, shared and constructor validation, Controller publication, policy stamping, compilation logging, gate precedence, ordered async dispatch, every lifecycle caller and tests. The target baseline remained stable across external HEAD changes. Decision: change; moderate one-file correction.
- Defect: invalid GeoIP already installed a front gate, but invalid GCIR was skipped and IpRestriction/ConnectionRateLimit denied only in their configured positions. This could delete a security policy or consume earlier persistent quota before an inevitable rejection. IpRestriction also needed Controller parity for captured legacy HTTP fields.
- Implementation: every enabled-invalid variant now records a bounded cold error and contributes a fixed reason to one general `FailClosedStreamPlugin` inserted at index zero. Explicit policy Block remains dominant; disabled and policy-Bypass entries remain inert; valid plugin order is unchanged. Shared validation prechecks IpRestriction and ConnectionRateLimit, including legacy IpRestriction fields, while GCIR and GeoIP constructor failures use the same helper.
- Logging: the registered cold runtime constructor emits only bounded plugin type, safe resource identity and a static error class. Raw validation details, legacy fields and configuration values are never logged. No data-path tracing or new metrics were added.
- Preserved contracts: all lifecycle callers, `DirectPeerIp`, immutable hot reload, sequential valid dispatch, cancellation, policy status handling, protocol logging and the constructors' standalone defensive validation are unchanged.
- Deferred: no new issue. D0101 remains the later authorized invalid-generation live fail-close/recovery integration seam.
- Review: `b0135_final_review` independently verified all variants and priorities. It required adding `legacy_http_fields()` to IpRestriction prevalidation and a discriminating serde/state-consumption regression; the corrected diff received LGTM with no remaining Must Fix or Should Fix.
- Verification: all 10 stream-runtime tests passed. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only and diff guards passed; Clippy retained only existing warnings.
- External gate support: commit `9b6b6fa0` changed 14 integration paths (11 added, three modified); the independently reconstructed 3,104-path manifest matches byte-for-byte and no integration worktree edit exists. External commit `1c55ab7b` introduced canonical links to the previously missing blocked rustls task; the added task restores link validation and grants no implementation authority.
- Commit: `c4805644`; never pushed by the campaign.
