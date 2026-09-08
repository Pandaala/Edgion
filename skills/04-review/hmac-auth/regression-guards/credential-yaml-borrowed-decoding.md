---
name: credential-yaml-borrowed-decoding
description: Review HMAC Secret credential-list format, YAML decoding ownership, and UTF-8 failure behavior.
knowledge_type: regression-guards
---

# HMAC credential YAML decoding borrows the fetched Secret

## Conclusion and scope

Controller `resolve_credential/hmac.rs::resolve_group` validates UTF-8 with
`std::str::from_utf8(&credentials_yaml_bytes.0)` and passes the resulting `&str`
to the YAML parser. Both standalone HmacAuth and shared HMAC conditions use this
resolver. D0036 removes one avoidable full-YAML copy per fetched credential Secret;
it does not remove all credential copies or establish allocator erasure.

## Rationale and evidence

`SecretStore::get` clones the stored Secret. `fetch_secret` returns that owned
Secret after reference registration and grant checks. The local Secret owns
`data["credentials.yaml"]` throughout synchronous decoding and parsing. Previously,
`String::from_utf8(bytes.clone())` allocated another complete credential buffer
solely to validate and expose UTF-8. Borrowing removes that extra owner without
holding the SecretStore lock or changing its API.

## Credential-list format and documentation boundary

HmacAuth Secrets contain a YAML list under the fixed `credentials.yaml` data key,
including for a single user. Operators may author that list as a block scalar in
`stringData["credentials.yaml"]`. `SecretHandler::parse` calls
`normalize_string_data`, which copies each string value into the same `data` key;
it does not synthesize a credential list from top-level `username` / `secret` keys.

`resolve_group` requires `data["credentials.yaml"]`, decodes UTF-8, and parses
`Vec<serde_yaml::Value>`. `HmacMatchConfig.username_field` and `secret_field` select
keys inside each mapping entry. The username becomes a `resolved_credentials` map
key; `HmacCredential.secret` holds the trimmed secret bytes and `group` records
the referring group's name. Custom entry field names do not change the outer key.

The D0036 follow-up `hmac-auth-quick-start-secret-format` found both language
guides using unsupported top-level `stringData.username` / `stringData.secret`.
That layout leaves the fixed key absent, so the resolver skips the Secret and
produces no credentials from it. The correction changes the English and Chinese
Quick Start examples to a one-entry list and clarifies the field selectors. It
does not add another input format or change runtime authentication semantics.

## Ownership boundaries and limits

- The store retains its Secret until replacement/removal or store destruction.
- The fetched Secret owns a separate snapshot until the loop iteration exits.
  The borrowed YAML view cannot outlive it; decoding and parsing do not await.
- `Vec<serde_yaml::Value>` owns parsed entries; entries are consumed in the loop.
- Retained `HmacCredential.secret`, `headers`, map keys, and `group` remain owned
  by the resolved configuration. Their downstream generations are outside this fix.
- Invalid or incomplete UTF-8 still warns and skips the Secret before YAML parsing;
  the error holds offset/length information rather than the credential buffer.

These are owner/release-event boundaries, not wall-clock or zeroization guarantees.
Store copies, YAML allocations, rejected duplicate material, transport buffers,
and retired generations follow the separate ownership evidence linked below.

## Verification and re-evaluation

`test_parse_resolves_hmac_credentials_and_registers_refs` covers malformed and
truncated UTF-8, safe decode diagnostics, reference registration, and valid
credentials following invalid Secrets. Existing HMAC tests cover shared conditions,
first-wins, metadata policy, and re-resolution. Tests verify semantics; the removed
clone is established by source inspection, not an allocation-count benchmark.
Re-evaluate if fetching becomes borrowed, decoding becomes asynchronous, the parser
retains borrowed values, or resolved credential ownership changes. Fresh execution
results are recorded in the ownership evidence.

Fresh verification for the documentation follow-up:

- `cargo test -p edgion-controller --lib test_parse_resolves_hmac_credentials_and_registers_refs`
  passed (1 test), exercising the current Controller credential resolver.
- `cargo test -p edgion-controller --lib normalize_string_data` passed (3 tests),
  covering stringData normalization, overwrite precedence, and absent input.
- A one-off PyYAML check parsed every YAML block on both user-guide pages and
  verified the single Secret's fixed key, one-entry list, username, minimum byte
  length, plugin reference, group name, and exact bilingual Secret parity.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed; clippy emitted warnings in
  existing Rust code outside this documentation change.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py` under `cicd/checks/` passed.

These checks plus source tracing cover the documented Secret's normalization and
resolution path; they are not a live Kubernetes apply or an end-to-end signed
request test. No runtime or integration-test files changed. Re-evaluate the
examples if the fixed key, list shape, entry selectors, or normalization changes.

## Resolution references

- D0036 — scoped copy reduction; [broader ownership evidence and remaining work](../../security/review-evidence/credential-copy-ownership.md).
- `hmac-auth-quick-start-secret-format` — D0036 documentation follow-up; corrected
  Secret layout in both HmacAuth user guides; docs-only disposition.
