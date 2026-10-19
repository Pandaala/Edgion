---
name: auth-challenge-realm-quoted-strings
description: Review configurable authentication realms, shared validation, and WWW-Authenticate quoted-string construction.
knowledge_type: regression-guards
---

# Authentication challenge realm quoted strings

## Conclusion and scope

The Gateway's canonical `send_gateway_auth_error_response` validates the raw realm
and escapes double quotes and backslashes before writing `WWW-Authenticate`.
The six configurable consumers are BasicAuth, KeyAuth, JwtAuth, OpenidConnect,
LdapAuth, and HmacAuth. Their `realm: String` fields retain the original value
through configuration serialization and runtime ownership; escaping occurs only
at response construction. HeaderCertAuth and JweDecrypt pass the constant `edgion`.

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
- Response path: `plugins/http/common/auth_common.rs` revalidates before response
  construction and reuses `escape_quoted_string` for realm and error description.
  Ordinary values are borrowed to avoid an extra escaped-string allocation.

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

## Exact-capacity challenge ownership

The shared response helper retains realm validation/escaping before constructing
the response header. Its private challenge builder sizes the output from the
already escaped realm and, only when an error code exists, the escaped description.
Literal separators, raw scheme/code bytes and both challenge forms remain the
same. The completed String moves into WWW-Authenticate instead of being copied
through a borrowed conversion. Response header/body/shutdown ordering is unchanged.

The byte-parity matrix compares the previous formatting expressions across 1,680
combinations, checks exact capacity and compares owned/borrowed header validation.
Valid owned conversions retain the source pointer and all measured conversions
allocate zero times on pinned http/bytes. Selected clean-description cases build
the challenge in one allocation requesting exactly its length; descriptions that
require escaping can allocate separately. A None code does not escape the body.
Existing async regressions retain realm rejection before writes and exact response
headers, body and write ordering. This is not an allocation-free response callback
or a throughput claim.
