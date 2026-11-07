---
name: allowed-san-publication-prepared-matcher
description: Use when reviewing EdgionTls mTLS SAN allowlist preparation at Gateway publication, the handshake matcher, or fail-closed handling of unprepared entries.
knowledge_type: regression-guards
---

# Allowed SAN matcher prepared at publication

## Conclusion and scope

Every `EdgionTls.spec.clientAuth.allowedSans[]` entry is validated and normalized
once per published generation. The handshake only consumes the result.

- `AllowedSan::compile` (`edgion-resources/src/resources/allowed_san.rs`) clears
  `compiled`, runs `allowed_san_schema_error`, and on success stores one
  `CompiledAllowedSan`: the trimmed OID (only for `OtherName`) and a
  `CompiledSanMatcher` (`DnsExact` trimmed value, `IpExact` parsed `IpAddr`,
  `Text { kind, value, ignore_case }` with a trimmed value, or a bounded `Regex`).
  On error, `compiled` stays `None`.
- `compiled` is `#[serde(skip)]` + `#[schemars(skip)]`: it never enters the CRD
  or conf_sync and is rebuilt after deserialize.
- Gateway `TlsStore::validated_entry` calls `compile_allowed_sans_for_runtime`
  for every non-empty allowlist, before validation and publication. Full,
  incremental and last-known-good publication carry the prepared
  `Arc<EdgionTls>` with its generation.
- `edgion_common::allowed_san::client_sans_match_allowed` does no config
  validation, trimming, allowed-IP parsing or regex compilation. An empty list
  is unconstrained; if any entry has `compiled == None` (invalid, uncompiled
  regex, or never prepared), the whole list rejects, even if another entry
  would match.

Matching semantics are unchanged: typed SAN isolation, OtherName OID equality,
RFC 6125 DNS wildcards in both directions, canonical IP equality without
IPv4-mapped equivalence, ASCII ignore-case for string operators, regex case
flags and bounds, SAN-before-CN ordering and sensitive failure diagnostics.

## Rationale and evidence

tls-01 found that every mTLS handshake with a SAN allowlist re-ran
`allowed_san_schema_error` for every entry (trim, OID grammar, a hostname regex
match for DNS Exact, an IP parse for IP Exact, and an error `String` on
failure), then re-trimmed values and re-parsed IP Exact operands per
comparison. The inputs are immutable per generation, and Gateway `validate_cert`
already rejects invalid entries via `allowed_san_config_error`. Publication-time
preparation follows
[design rule U](../../../00-design-rules.md#u-gateway-hot-path-performance--bounded-memory)
and extends the regex-only preparation described in
[allowed SAN regex case mode](allowed-san-regex-case-mode.md).

The only production handshake consumer is the BoringSSL verify callback
(`mtls_verify_callback.rs::validate_certificate_whitelist` →
`validate_san_whitelist`), which reads the `EdgionTls` held by a
`CompiledEdgionTls`. That is only built by `TlsStore` from the prepared resource.

## Alternatives and limits

The originally proposed design kept the old per-entry validation as a fallback
for unprepared values and added several derived fields plus a list-level
verdict. It was rejected: two matching paths would need permanent parity tests,
and five derived values would have to stay consistent. Instead, a single
`compiled` field carries the invariant "`Some` ⇔ valid and prepared", and an
unprepared entry fails closed. This is the same rule regex entries already
followed. A future consumer that forgets to prepare rejects every client, which
is loud and safe rather than a bypass.

Prepared state is a snapshot of the public fields at `compile` time. Code that
mutates an entry after compiling must call `compile` again. Presented IP SANs
are still parsed per comparison.

## Verification and re-evaluation

Current-run verification:

- `cargo test -p edgion-resources --lib allowed_san`: 15 passed (normalized
  operands, non-Exact DNS/IP dispatch to text matchers, schema-error parity
  with admission and stale-state clearing, serde round-trip then recompile).
- `cargo test -p edgion-common --lib allowed_san`: 18 passed (invalid entry plus
  matching entry rejects, unprepared entry of every kind fails closed, trimmed
  operands, non-Exact DNS/IP string semantics).
- `cargo test -p edgion-gateway --lib -- tls::store tls::validation::mtls
  tls::boringssl::mtls_verify_callback tls::runtime::gateway::tls_pingora`:
  69 passed, including
  `non_regex_allowed_sans_are_prepared_on_publication_and_kept_by_last_known_good`
  and real DNS-vs-URI type isolation plus invalid-entry whole-list rejection
  in the verify callback.
- `cargo clippy --workspace --all-targets` passed with no warnings in changed
  files. Integration suites were not run; matching behavior is unchanged and the
  existing `EdgionTls/mTLS` allowlist fixtures are schema-valid.

Re-evaluate if a new consumer matches `allowedSans` outside `TlsStore`
publication, if `AllowedSan` gains a field the matcher reads directly, or if
publication stops rebuilding skipped fields after deserialize.

## Resolution references

- tls-01 / `tls-01-prepared-san-policy` (2026-11-07): per-handshake allowlist
  validation moved to publication through a unified prepared matcher.
