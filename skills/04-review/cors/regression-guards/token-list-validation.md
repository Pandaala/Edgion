---
name: token-list-validation
description: Review EdgionPlugins CORS method and header list validation without rejecting valid extension tokens or formatting.
knowledge_type: regression-guards
---

# CORS method and header token-list validation

## Conclusion and scope

`CorsConfig::validation_error` validates the non-empty members of `allow_methods`,
`allow_headers`, and `expose_headers` with the canonical
`gwapi_types::is_valid_header_token`. Methods and field names share the token
character set. `validate_and_init` calls this same stateless validator before
building origin matchers, eliminating the duplicated credential guards.

This applies to EdgionPlugins CORS only, including Controller ingest and Gateway
construction after conf_sync deserialization. Native Gateway API CORS is separate.

## Rationale and evidence

Previously arbitrary method/header strings passed validation and reached
response generation. For example `GET POST` is one invalid token, whereas
`GET, POST` is a valid two-member list. An injected colon or CR/LF also must not
pass list validation. Errors identify the field and one-based member index,
without copying the operator's value into a diagnostic.

The grammar follows [Fetch CORS header syntax](https://fetch.spec.whatwg.org/#http-new-header-syntax)
and [HTTP token/list syntax](https://www.rfc-editor.org/rfc/rfc9110.html#section-5.6).
The user explicitly required conservative validation: do not block valid
configurations merely because their tokens are uncommon.

- Accept extension methods, mixed case, all token punctuation, duplicates,
  empty lists, and empty members. Ignore only SP and HTAB around each member
  for validation; never use Unicode whitespace trimming to hide invalid bytes.
- Preserve the stored string, order, and case; add no fixed method allowlist or
  length restriction. Empty-member acceptance is deliberately permissive,
  following recipient list parsing rather than imposing canonical formatting
  on configuration strings.
- Preserve exact-match `*`/`**` handling and existing credentials restrictions.
  A token containing `*` does not gain a new pattern-matching meaning.
- `EdgionPlugins::validate_plugins` exposes errors for enabled entries;
  `PluginRuntime::add_from_request_filters` skips invalid construction and
  `seal_fail_closed` inserts `RejectInvalidConfig`. Disabled entries remain inert.

## Alternatives and limits

Do not use an enum of common HTTP methods, restrict headers to alphanumeric plus
hyphen, reject empty lists, normalize case, or enforce Gateway API field-length
limits on these EdgionPlugins strings. Those would reject supported inputs or
change wire values beyond this fix.

Shared input checks do not imply complete regex compilation parity: stateless
validation checks individual patterns, while initialization still builds merged
regexes. No runtime-cache or request-path optimization is claimed. Remaining
work is tracked in [header precomputation](../../../../tasks/todo/precompute-cors-response-headers.md).

## Verification and re-evaluation

- `cargo test -p edgion-resources --lib resources::edgion_plugins::plugin_configs::cors::tests`:
  33 passed, including valid/invalid list matrices and entry-point error equality.
- `cargo test -p edgion-gateway --lib cors`: 36 passed, including resource-ingest
  parity, fail-closed construction, and disabled invalid entries.
- `target/debug/test_client -g -r EdgionPlugins -i Cors`: 15 passed on the shared
  Controller/Gateway topology with HTTPRoute/Basic, EdgionPlugins/base, and
  EdgionPlugins/Cors fixtures.
- `target/debug/test_client -g --dynamic-scenario CorsValidation`: passed on the
  same topology shape. Covers valid -> invalid method/header/expose lists ->
  disabled -> valid recovery, checking Controller status, Gateway responses,
  exact valid wire values, and cleanup. The assertion respects omission of
  default `RequestFilterEntry.enable: true` from serialized resources.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed. Clippy reports two existing
  test warnings outside this change (`dsl/grpc_error.rs` redundant async block
  and Kafka `client.rs` err/expect); no new warning was introduced.
- Agent-doc, SSA force, metrics inventory, Pingora source policy, Gateway API
  transform-contract, and Gateway no-tracing guards passed. No schema fields
  changed; no full-workspace test run was requested or performed.

Re-evaluate when changing token grammar, credential policy, wildcard expansion,
runtime initialization, or output normalization. The referenced constructor and
preflight regression guards remain applicable.

## Resolution references

- `unify-cors-validation-and-header-precomputation.md`: real validation defect;
  scoped fix authorized with conservative input acceptance. Precomputation is
  deferred in the linked task, not included in this correction.
