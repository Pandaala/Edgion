---
name: auth-challenge-realm-quoted-strings
description: Review configurable authentication realms, shared validation, and WWW-Authenticate quoted-string construction.
knowledge_type: regression-guards
---

# Authentication challenge realm quoted strings

## Conclusion and scope

The Gateway's canonical `PreparedAuthChallenge` validates and escapes the raw realm
once during plugin construction. Its immutable header base lives with the plugin's
runtime generation. `send_gateway_auth_error_response` borrows this owner: None-code
responses clone the base; Some-code responses append request-specific error fields.
The six configurable consumers are BasicAuth, KeyAuth, JwtAuth, OpenidConnect,
LdapAuth, and HmacAuth; HeaderCertAuth and JweDecrypt prepare constant `edgion`.
Serialized raw configuration is unchanged. LDAP's configured header_type remains
the exact challenge scheme. Runtime-only raw realm copies in KeyAuth, HmacAuth and
LDAP AuthPolicy are replaced by the prepared owner.

The shared `validate_auth_realm` in
`edgion-resources/src/resources/edgion_plugins/auth_realm.rs` rejects ASCII controls
(`0x00`–`0x1F`) and DEL (`0x7F`) without echoing realm contents. Rejecting HTAB is an
explicit product restriction; RFC 9110 quoted strings themselves permit HTAB.
All other UTF-8 bytes are preserved. Existing plugin-specific defaults and
empty-value checks remain unchanged. The helper's existing error-description
escaping still removes controls; realm validation never silently removes them.

## Rationale and evidence

D0039, originally deferred from B0067 on 2026-09-01, identified direct realm
interpolation into a quoted string while only `error_description` was escaped.
An operator value such as `Partner "API"` broke the quoted boundary; a backslash
could change the parsed value or escape the closing delimiter. This is a real
configuration-triggered correctness/security defect, not proof of remote
authentication bypass or response splitting.

- Protocol syntax: [RFC 9110 section 5.6.4](https://www.rfc-editor.org/rfc/rfc9110.html#section-5.6.4).
- Config path: each owning config validator calls `validate_auth_realm`;
  `EdgionPlugin::validation_error` and `EdgionPlugins::validate_plugins` surface it
  to the Controller's existing `Accepted=False` status handling.
- Gateway path: `PluginRuntime::add_from_request_filters` validates before
  construction; `compile_plugins` seals errors into the existing fail-closed gate.
  Invalid auth updates replace the previous generation with that gate; they do
  not use the DSL/WAF last-known-good exception. Disabled entries remain inert.
- Construction path: `plugins/http/common/auth_common.rs` validates the realm and
  reuses `escape_quoted_string`. Invalid direct construction retains a data-free
  error; the response path checks it before any write, with no panic or fallback.
- Response descriptions still use the same canonical escaping per request. Prepared
  base bytes are copied as bytes for dynamic suffixes, preserving UTF-8 obs-text.

## Alternatives and limits

Plugin-local escaping was rejected because all consumers need one wire contract.
Silently stripping realm controls was rejected because it changes protection-domain
identity and hides operator mistakes. Header-value validation alone cannot enforce
quoted-string boundaries. Scheme selection, error codes/descriptions, status/body,
failure delays, and access-log diagnostics are outside this correction.

The earlier [Bearer error-attribute correction](../../jwt-auth/regression-guards/bearer-www-authenticate-error-attributes.md)
remains applicable to error attributes, but did not establish realm safety.
No schema fields, serialized secrets, or CRD field shapes change.

## Verification and re-evaluation

Verified on 2026-09-07:

- `cargo test -p edgion-resources --lib auth_realm`: 3 passed.
- `cargo test -p edgion-resources --lib auth`: 137 passed.
- `cargo test -p edgion-resources --lib openid_connect`: 19 passed.
- `cargo test -p edgion-gateway --lib auth_realm`: 5 passed.
- `cargo test -p edgion-gateway --lib plugins::http::common::auth_common::tests`:
  19 passed. These command filters overlap; counts are not additive.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed. Clippy reported existing
  warnings outside the changed files. The first workspace check crossed concurrent
  repository commits and failed to resolve an already-added dependency; rerunning
  against the updated manifest passed without changing unrelated code.
- `test_client -g --dynamic-scenario BasicAuthDynamic`: passed against the shared
  local Controller/Gateway/test_server topology, loaded with
  `base,HTTPRoute/Basic,EdgionPlugins/base,EdgionPlugins/BasicAuth` through
  `start_all_with_conf.sh`. Startup and scenario execution shared one command
  session. Earlier incomplete-fixture runs were setup failures, not passes.
- Independent read-only review: LGTM after correcting the test fixtures to include
  LDAP/OIDC required configuration fields.
- Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway no-tracing guards, and the final formatting check: passed.

Coverage includes all six validators, wire round trips, every ASCII control,
escaped challenge bytes, unchanged response ordering/body, invalid update
publication and recovery, and disabled entries. The dynamic update checks an
actual challenge after changing to a realm with quotes and a trailing backslash.
The matching Kubernetes fixture was updated; a Kubernetes cluster run was not
performed. Full workspace unit tests were not run.

Re-evaluate when adding a realm-bearing plugin, changing HTTP serialization,
altering accepted character/empty-value policy, or bypassing shared configuration
validation. Historical checks from the Bearer attribute fix are not current tests.

## Resolution references

- D0039 / `harden-auth-challenge-realm-quoted-strings`: corrected and verified on
  2026-09-07; the original disposition was deferred, not accepted risk.

## Generation-owned challenge preparation

`auth-02-prepared-challenge-generation` moves only stable scheme/realm work into
the runtime constructor. It does not cache status, body, error code, description or
secrets. One prepared owner contains a valid HeaderValue or a static failure.
The constructor promotes shared byte ownership on the cold path; no mutable
response template, global cache or extra per-request lookup exists. Existing entry
and request-pinned runtime ownership keeps old/new generations coherent and releases
prepared data after their final owner and response header clone drop.

Some-code construction sizes one output Vec from base bytes, raw code and escaped
description, then transfers it into HeaderValue. None-code paths do not inspect the
body or append attributes. Existing response ordering, failures, delay and shutdown
semantics remain unchanged. HeaderValue::to_str must not be used on the prepared
base because accepted UTF-8 realms can contain obs-text.

The byte-parity and exact-capacity regressions remain, with first/repeated clone,
concurrent dynamic descriptions, invalid direct construction and generation
ownership coverage. Allocation assertions describe challenge construction only;
the complete response still allocates. Retained base bytes and shared metadata are
an explicit per-generation tradeoff, not a throughput or RSS claim.
