# B0082 — Shared synchronization and ExtProc protocol contracts

## Scope and outcome

Complete source review: conf_sync/{config,mod,payload,proto,traits,types}.rs,
conf_sync/proto/config_sync.proto, ext_proc/{mod,proto}.rs and
ext_proc/proto/ext_proc.proto. Complete tests: tests/unit/conf_sync/{config,payload}_test.rs
and tests/unit/ext_proc/mod_test.rs. All paths are under edgion-common.

Only config.rs comments change: omission of transport configuration is fail-close;
absence of a selected file TLS config does not authorize plaintext. Existing
legacy tls-field cleanup remains separate debt because it touches frozen fixtures.
The other twelve files need no change. Codec checks retain 512 MiB payload and
128 MiB frame-window bounds, exact decoded length, one complete frame, retained
blocking-job permits on cancellation, and generation cancellation checks around decoding. Trait
ownership and wire numbers match consumers. ExtProc is a trimmed wire-compatible
surface, not a claim of full Envoy behavior support.

## Evidence

Primary full-file review plus independent `sync_review` and `watch_review`.
Both reviewers independently read the original rules and bounded-payload,
plaintext-fail-close, Watch recovery, ExtProc phase/replacement/header knowledge.
The first reviewer additionally checked upstream Envoy v1.37.0 field and enum
numbers. Both returned LGTM on the final comments-only diff.
Targeted tests and shared final gates passed as recorded below. No conformance run claimed.

## Verification on 2026-09-27

Common library suite: 314 passed, 4 pre-existing ignored. Controller CA suite:
5 passed, including the new overflow regression. Workspace all-target check and
Clippy passed; fmt, agent docs, SSA, metrics inventory, Pingora source policy,
Gateway API transform, unit-test layout and Gateway tracing guards passed.
All 3,772 frozen integration fingerprints match. No end-to-end/conformance run
is claimed; production change is bounded to pure CA date arithmetic.

Fresh test rebuild initially lacked protoc; installed protoc-wheel-0 29.3 only
under /tmp and supplied PROTOC explicitly. The transform guard used the existing
/tmp validation Python environment for PyYAML. No repository dependency or system
configuration was changed to repair the build environment.
