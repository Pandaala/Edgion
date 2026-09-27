# B0081 — Common certificate duration

## Scope

- Baseline: `5339cb5ef`.
- Pipeline state: terminal.
- Anchor: `edgion-common`.
- Target: `edgion-common/src/conf_sync/cert_duration.rs` and complete associated unit-test context.
- Initial analysis was read-only; the continuation below records the necessary Controller support edits. Protected paths and frozen integration files apply.
- Promote before B0080 to preserve lexical file order; batch identifiers reflect analysis allocation only.

## Context and value gate

The parser preserves the dedicated auto-CA h/d/w/mo/y contract and rejects
integer overflow. The current `from_hours` consumers supply valid constants.
However, `ca_ttl: "10000000y"` parses and passes numeric validation while
`OffsetDateTime::now_utc() + ttl` panics in downstream CA issuance.

The necessary support scope is exactly
`edgion-controller/src/services/conf_sync_ca/ca.rs` and
`edgion-controller/tests/unit/services/conf_sync_ca/ca_test.rs`.
Root generation/renewal and server/client leaf issuance now use checked date
addition and propagate contextual errors through the existing Result paths.
No duration grammar, policy default, schema or valid-certificate contract changes.
The new regression uses the accepted configuration and exercises all four APIs.

Independent reviewers `cert_review` and `watch_review` inspected the complete
code/test diff, original rules, and provisioning/rotation/identity-sync callers;
both returned LGTM. Targeted runtime validation and shared commit gates passed as recorded below.

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
