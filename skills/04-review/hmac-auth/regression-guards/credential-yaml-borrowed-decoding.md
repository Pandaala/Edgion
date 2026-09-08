---
name: credential-yaml-borrowed-decoding
description: Review HMAC Secret YAML decoding ownership and UTF-8 failure behavior.
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
and retired generations still require the broader audit.

## Verification and re-evaluation

`test_parse_resolves_hmac_credentials_and_registers_refs` covers malformed and
truncated UTF-8, safe decode diagnostics, reference registration, and valid
credentials following invalid Secrets. Existing HMAC tests cover shared conditions,
first-wins, metadata policy, and re-resolution. Tests verify semantics; the removed
clone is established by source inspection, not an allocation-count benchmark.
Re-evaluate if fetching becomes borrowed, decoding becomes asynchronous, the parser
retains borrowed values, or resolved credential ownership changes. Fresh execution
results are recorded in the active task.

## Resolution references

- D0036 — scoped copy reduction; [the full audit remains open](../../../../tasks/todo/audit-auth-credential-copy-lifetimes.md).
