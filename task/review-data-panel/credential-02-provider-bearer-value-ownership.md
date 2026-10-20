# Provider Bearer header value ownership

Status: deferred ownership and sensitivity decision for the user.
Source baseline: `8c92ba383`. No runtime change.

## Current evidence

`ai/connectors/provider.rs::prepare_provider_request` formats a Bearer String for
OpenAI and OpenAICompatible, then passes a borrowed view to `set_request_header`.
PingoraSessionAdapter copies that value with `to_string()` before insertion.
The pinned http 1.5.0 `TryFrom<String> for HeaderValue` consumes the copied
buffer through Bytes and validates its bytes; the provider's original formatted
String is subsequently dropped.

The existing typed setter can consume a HeaderValue built from the original
String, avoiding that second payload copy. Its documented contract requires
credential producers to mark values sensitive. The current String conversion
initializes `is_sensitive` to false, and this provider path does not set it.
Setting the flag during the ownership change is observable metadata behavior:
the canonical LinkSys template engine propagates a source header's sensitivity,
and typed Debug rendering observes it. Keeping false in a newly typed credential
call would conflict with that API contract.

This is not proof that a credential currently reaches an unsafe log sink.
Existing name-based redaction remains independently required. The sensitivity
scope needs explicit review rather than being hidden in a behavior-preserving
allocation change.

The fixed authorization field is NOT protected by the gRPC transcoder's sealed
header list. Therefore guard-before-parse ordering is not an obstacle to this
specific field. It still matters if a proposed solution expands into a generic
owned-string setter for protected fields.

## Decision

Choose whether to authorize a local owned HeaderValue handoff together with an
explicit credential-sensitivity correction and its consumer audit, or retain the
current interface pending that review. A generic owned-string setter would
expand the common session API and must preserve its existing guards and mutation
order; it is not selected here. Publication-time prepared credential values
would introduce secret-generation ownership and retention questions beyond the
local move, and are not automatically authorized by this task.

## Acceptance for an approved implementation

- Use the canonical session adapter and pinned HeaderValue validation. Preserve
  exact Bearer bytes, replace-all behavior, invalid-value failure mapping and
  canonical credential isolation. Do not log the input or decoded secret.
- Define the sensitive flag explicitly and verify propagation through the real
  adapter, cloning, template composition and Debug redaction. Retain independent
  access-log/name-based redaction and final Connection credential checks.
- Test empty, ordinary, long and invalid control-character credentials. At the
  setter boundary failed insertion must not clear deferred mutations or replace
  existing header values. Whole-provider preparation already scrubs credential
  headers/deferred mutations and updates URI/host before injection; preserve that
  ordering rather than claiming or adding whole-operation atomicity.
- Exercise request rebuilding across retries and credential rotation with the
  selected generation. Add no global credential cache, second credential owner
  or longer-lived prepared secret without separate retention evidence.
- Keep native Anthropic and other string/typed setter consumers unchanged unless
  explicitly included in the approved sensitivity scope. A generic setter design
  must separately cover sealed RPC fields and their rejection latch.
- Compare allocation requests and copied bytes with the current provider path,
  excluding fixture setup. Distinguish local value handoff savings from header
  maps, name handling, URI parsing and full request preparation.
- Use a new IMPL plan and exact implementation allow-list.

## Knowledge and verification limits

The typed setter contract is in `plugins/runtime/traits/session.rs`.
[Template sensitivity propagation](../../skills/04-review/plugin-runtime/regression-guards/request-template-policy-unification.md#deep-review-corrections)
records why metadata matters. The current implementation is in
`link_sys/template/engine.rs`, with real-adapter coverage in
`tests/unit/plugins/runtime/session_adapter_test/record_outcome_tests.rs`.

Fresh baseline verification passed 49 session-adapter, five provider-connector
and 36 dispatch tests (90 total), formatting and workspace all-target
check/Clippy. Existing unrelated Clippy warnings remain. These tests verify the
current path, not a selected sensitivity correction or copy-removal design.
No allocation benchmark, live provider exchange or demonstrated leak was produced.
