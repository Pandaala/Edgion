---
name: borrowed-request-header-access
description: Review borrowed PluginSession request headers and authentication parsing ownership.
knowledge_type: regression-guards
---

# Borrowed request-header access

## Conclusion and scope

Authentication parsers can read the original request header without first copying
the complete Authorization value into a String. The shared boundary is
`PluginSession::request_header_map() -> &http::HeaderMap`, with
`PluginSessionHeaderExt::header_str() -> Option<&str>` derived from it.
BasicAuth, LDAP, HMAC, JWT, OIDC, and JWE use this borrowed tier when extracting
their configured authentication header. This corrects the premise of
`auth-borrowed-authorization-header` (the D0035 follow-up); it does not establish
allocation-free authentication or credential erasure.

## Rationale and evidence

The former path called `header_value`, whose Pingora adapter constructs a String
before parsing. Commit `b55375ca` introduced the common borrowed boundary and
migrated the affected consumers. Current source confirms the complete chain:

- `edgion-gateway/src/plugins/runtime/traits/session.rs`: the shared trait
  exposes the live request HeaderMap by reference.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`: the production adapter
  returns `&self.inner.req_header().headers` without an intermediate owner.
- `edgion-gateway/src/plugins/runtime/traits/header_ext.rs`: `header_str` uses
  `get(name).and_then(|value| value.to_str().ok())` with no allocation.
- `http/basic_auth/plugin.rs`, `http/ldap_auth/plugin.rs`, and
  `http/hmac_auth/plugin.rs` under the gateway plugins directory parse borrowed
  Authorization or Proxy-Authorization text. LDAP retains proxy-first selection;
  HMAC retains authorization-first selection.
- `http/jwt_auth/request_stage.rs`, `http/openid_connect/callback.rs`, and
  `http/jwe_decrypt/plugin.rs` borrow the header before creating the selected
  token's owned result.
- `runtime/traits/test_helpers.rs` supplies all mock header tiers from one
  configured HeaderMap; the production path does not pay for these test copies.

This follows [design rules](../../../00-design-rules.md) and the repository rule
that data-plane performance outranks mock convenience. The extension trait keeps
borrowed optional returns outside the automocked trait; it is not a plugin-local
adapter bypass.

## Alternatives and limits

`header_value` remains available for consumers requiring independent ownership.
The borrow is tied to the session and cannot coexist with conflicting mutable
session access. An await alone does not universally require an owned copy;
ownership depends on the consumer's actual lifetime and task boundary.

The borrowed text tier preserves the owned tier's first-value lookup,
case-insensitive names, and `HeaderValue::to_str` rejection behavior. Raw header
presence is not equivalent to successful text access. Deferred upstream
credential headers are invisible to both request-header tiers.

Decoded Basic/LDAP credentials, extracted JWT/OIDC/JWE tokens, crypto internals,
and KeyGet results may still own bytes. The broader questions and their scoped
no-code disposition are maintained in the
[credential copy retention decision](../../security/intentional-design/credential-copy-retention.md).
Dropping any owner is not proof that allocator or transport memory was wiped.

## Verification and re-evaluation

Source inspection for this closure confirmed the adapter and consumer chain.
Existing `header_ext` unit tests cover trait-object access, case-insensitive and
missing lookup, rejected text with raw-byte access, and duplicate first-value /
per-name insertion order. This documentation-only closure does not change or
rerun authentication integration scenarios; historical tests are not fresh runs.
Fresh closure verification on 2026-09-11 passed: `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
agent-doc validation, SSA-force, metrics-inventory, Pingora-source-policy, and
`git diff --check`. Clippy reported existing workspace warnings; this is not a
warning-free result. Independent read-only source and knowledge review returned
LGTM. No runtime tests or full `cargo test --all` were run for this no-code closure.

Agent-doc validation initially found an unrelated missing header-preparation
follow-up referenced by the existing header modifier guard. Its pending tracking
record was restored from that guard and current source, independently reviewed,
and validation passed; no header-modifier implementation was changed.

Re-evaluate when the session adapter, header accessor, auth extraction paths,
mock source of truth, or text/duplicate selection semantics change. Do not infer
that every request-header consumer or downstream credential copy was migrated.

## Resolution references

- `auth-borrowed-authorization-header` / D0035: already implemented; the user
  authorized no-code closure on 2026-09-11. The original task was still pending
  after the shared accessor and consumer migration.
